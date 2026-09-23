use openengine_cluster_protocol::{IDEMPOTENCY_REUSE, NOT_FOUND, RUN_CONFLICT};
use openengine_cluster_client::ClientError;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::NativeV2CliError;
use crate::native_v2_admission::NativeV2AdmissionError;
use crate::native_v2_delivery::GITHUB_TOKEN_ENV;
use crate::native_v2_supervisor::RunEnvironmentError;

pub const ERROR_FORMAT_ENV: &str = "ZEROSHOT_ERROR_FORMAT";
pub const JSON_ERROR_FORMAT: &str = "json";
const ERROR_SCHEMA: &str = "zeroshot.error/v1";
const CONNECTION_SET_HELP_COMMAND: &str = "zeroshot connection set --help";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DiagnosticKind {
    InvalidRequest,
    Protocol,
    RunNotFound,
    SubmissionConflict,
    Target,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeV2CliDiagnostic {
    schema: &'static str,
    kind: DiagnosticKind,
    code: String,
    message: String,
    path: Option<Vec<Value>>,
    node: Option<String>,
    details: Value,
}

impl NativeV2CliDiagnostic {
    pub fn target(message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Target, "target.unavailable", message)
    }

    fn new(kind: DiagnosticKind, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            schema: ERROR_SCHEMA,
            kind,
            code: code.into(),
            message: message.into(),
            path: None,
            node: None,
            details: json!({}),
        }
    }

    fn with_context(mut self, node: Option<String>, details: Value) -> Self {
        self.node = node;
        self.details = object_details(details);
        self
    }

    fn with_details(self, details: Value) -> Self {
        self.with_context(None, details)
    }
}

impl NativeV2CliError {
    #[must_use]
    pub fn diagnostic(&self) -> NativeV2CliDiagnostic {
        request_diagnostic(self)
            .or_else(|| run_diagnostic(self))
            .or_else(|| protocol_diagnostic(self))
            .unwrap_or_else(|| NativeV2CliDiagnostic::target(self.to_string()))
    }
}

fn request_diagnostic(error: &NativeV2CliError) -> Option<NativeV2CliDiagnostic> {
    let diagnostic = match error {
        NativeV2CliError::Usage(message) => request(usage_code(message), message, None, json!({})),
        NativeV2CliError::Json { kind, path, source } => request(
            json_code(source),
            error.to_string(),
            None,
            json!({
                "fileKind": kind,
                "filePath": path,
                "line": source.line(),
                "column": source.column(),
            }),
        ),
        NativeV2CliError::InitialInput(message) => request(
            "input.type_mismatch",
            error.to_string(),
            None,
            json!({"reason": message}),
        ),
        NativeV2CliError::InvalidRun(source) => request(
            admission_code(source),
            error.to_string(),
            admission_node(source),
            admission_details(source),
        ),
        NativeV2CliError::Environment(name) => request(
            "runtime.missing_environment",
            error.to_string(),
            None,
            json!({"environment": name.as_str()}),
        ),
        NativeV2CliError::RunEnvironment(source) => request(
            environment_code(source),
            error.to_string(),
            None,
            environment_details(source),
        ),
        NativeV2CliError::GitHubToken => request(
            "runtime.invalid_environment",
            error.to_string(),
            None,
            json!({}),
        ),
        NativeV2CliError::Update(_) => request("update.failed", error.to_string(), None, json!({})),
        _ => return None,
    };
    Some(diagnostic)
}

fn request(
    code: &str,
    message: impl Into<String>,
    node: Option<String>,
    details: Value,
) -> NativeV2CliDiagnostic {
    NativeV2CliDiagnostic::new(DiagnosticKind::InvalidRequest, code, message)
        .with_context(node, details)
}

fn usage_code(message: &str) -> &'static str {
    if message.contains("invalid value")
        && (message.contains("--template") || message.contains("<TEMPLATE>"))
    {
        "template.unknown"
    } else if message.contains("template delivery mode")
        || message.contains("not supported by --template")
    {
        "template.unsupported_delivery"
    } else {
        "request.invalid"
    }
}

fn json_code(source: &serde_json::Error) -> &'static str {
    if source.to_string().contains("unknown field") {
        "json.unknown_field"
    } else {
        "json.invalid"
    }
}

fn environment_code(source: &RunEnvironmentError) -> &'static str {
    match source {
        RunEnvironmentError::MissingConnection(_) | RunEnvironmentError::MissingField(_, _) => {
            "runtime.missing_environment"
        }
        _ => "runtime.invalid_environment",
    }
}

fn environment_details(source: &RunEnvironmentError) -> Value {
    match source {
        RunEnvironmentError::MissingConnection(key) => json!({
            "connection": key.as_str(),
            "helpCommand": CONNECTION_SET_HELP_COMMAND,
        }),
        RunEnvironmentError::MissingField(key, field) => json!({
            "connection": key.as_str(),
            "requiredFields": [field.as_str()],
            "helpCommand": CONNECTION_SET_HELP_COMMAND,
        }),
        _ => json!({}),
    }
}

fn admission_code(source: &NativeV2AdmissionError) -> &'static str {
    match source {
        NativeV2AdmissionError::InitialInput(_) => "input.type_mismatch",
        NativeV2AdmissionError::MissingRuntimeBinding { .. } => "runtime.missing_binding",
        NativeV2AdmissionError::UnexpectedRuntimeBinding { .. } => "runtime.unexpected_binding",
        NativeV2AdmissionError::MergePlanAgentGitHubToken { .. } => "runtime.invalid_environment",
        NativeV2AdmissionError::UnsupportedGraphProfile => "graph.unsupported_profile",
        _ => "request.invalid",
    }
}

fn admission_node(source: &NativeV2AdmissionError) -> Option<String> {
    match source {
        NativeV2AdmissionError::Attempts { node, .. }
        | NativeV2AdmissionError::MissingRuntimeBinding { node }
        | NativeV2AdmissionError::UnexpectedRuntimeBinding { node }
        | NativeV2AdmissionError::MissingAgentInstructions { node }
        | NativeV2AdmissionError::DeliveryInstructionsForbidden { node }
        | NativeV2AdmissionError::DeliveryMustBeVerifier { node }
        | NativeV2AdmissionError::UnsupportedDeliveryWorker { node, .. }
        | NativeV2AdmissionError::DeliveryWorkerRequiresBinding { node, .. }
        | NativeV2AdmissionError::InvalidDeliveryContract { node, .. }
        | NativeV2AdmissionError::MergePlanAgentGitHubToken { node } => Some(node.to_string()),
        _ => None,
    }
}

fn admission_details(source: &NativeV2AdmissionError) -> Value {
    match source {
        NativeV2AdmissionError::DeliveryNodeCount { found, .. } => json!({"found": found}),
        NativeV2AdmissionError::DeclaredEnvironmentTooLarge { found } => json!({"found": found}),
        NativeV2AdmissionError::MergePlanAgentGitHubToken { .. } => {
            json!({"environment": GITHUB_TOKEN_ENV})
        }
        _ => json!({}),
    }
}

fn run_diagnostic(error: &NativeV2CliError) -> Option<NativeV2CliDiagnostic> {
    match error {
        NativeV2CliError::RunNotFound { run_id } => Some(
            NativeV2CliDiagnostic::new(
                DiagnosticKind::RunNotFound,
                "run.not_found",
                error.to_string(),
            )
            .with_details(json!({"runId": run_id})),
        ),
        NativeV2CliError::SubmissionConflict { existing_run_id } => Some(
            NativeV2CliDiagnostic::new(
                DiagnosticKind::SubmissionConflict,
                "submission.conflict",
                error.to_string(),
            )
            .with_details(json!({"existingRunId": existing_run_id})),
        ),
        NativeV2CliError::Remote {
            code,
            message,
            details,
        } => Some(remote_diagnostic(code, message, details.clone())),
        _ => None,
    }
}

fn remote_diagnostic(code: &str, message: &str, details: Option<Value>) -> NativeV2CliDiagnostic {
    let mut details = object_details(details.unwrap_or_else(|| json!({})));
    if code.eq_ignore_ascii_case("connection_unavailable") {
        if let Value::Object(details) = &mut details {
            details
                .entry("helpCommand".to_owned())
                .or_insert_with(|| json!(CONNECTION_SET_HELP_COMMAND));
        }
    }
    match code {
        NOT_FOUND => {
            NativeV2CliDiagnostic::new(DiagnosticKind::RunNotFound, "run.not_found", message)
                .with_details(details)
        }
        RUN_CONFLICT | IDEMPOTENCY_REUSE => NativeV2CliDiagnostic::new(
            DiagnosticKind::SubmissionConflict,
            "submission.conflict",
            message,
        )
        .with_details(details),
        _ => NativeV2CliDiagnostic::new(DiagnosticKind::Target, code.to_lowercase(), message)
            .with_details(details),
    }
}

fn protocol_diagnostic(error: &NativeV2CliError) -> Option<NativeV2CliDiagnostic> {
    match error {
        NativeV2CliError::TargetTransport { name, origin, .. } => Some(
            NativeV2CliDiagnostic::target(error.to_string())
                .with_details(json!({"target": name, "origin": origin})),
        ),
        NativeV2CliError::Protocol(_) | NativeV2CliError::OutputJson(_) => {
            Some(NativeV2CliDiagnostic::new(
                DiagnosticKind::Protocol,
                "protocol.invalid",
                error.to_string(),
            ))
        }
        _ => None,
    }
}

fn object_details(details: Value) -> Value {
    match details {
        Value::Object(_) => details,
        value => Value::Object(Map::from_iter([("nativeDetails".to_owned(), value)])),
    }
}

pub(super) fn client_error(error: ClientError) -> NativeV2CliError {
    match error {
        ClientError::Backend(error) => NativeV2CliError::Remote {
            code: error.code,
            message: error.message,
            details: error.details,
        },
        ClientError::Rpc(error) => match error.data {
            Some(data) => NativeV2CliError::Remote {
                code: data.code,
                message: error.message,
                details: data.details,
            },
            None => NativeV2CliError::Protocol(error.message),
        },
        ClientError::Transport(_) => NativeV2CliError::Disconnected,
        error => NativeV2CliError::Protocol(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use openengine_cluster_client::TransportError;
    use openengine_cluster_protocol::{
        ConnectionKey, DomainErrorData, EnvironmentVariableName, JsonRpcError, NodeName,
    };
    use openengine_cluster_server::BackendError;
    use openengine_cluster_testkit::assertions::{AssertValue, JsonAt};
    use serde_json::json;

    use super::*;

    fn assert_diagnostic_cases<const N: usize>(cases: [(NativeV2CliError, &str, &str, Value); N]) {
        for (error, kind, code, details) in cases {
            let value = serde_json::to_value(error.diagnostic()).assert_value();
            assert_eq!(value.assert_key("kind"), kind);
            assert_eq!(value.assert_key("code"), code);
            assert_eq!(value.assert_key("details"), &details);
            assert!(value.assert_key("details").is_object());
        }
    }

    #[test]
    fn request_diagnostics_keep_stable_recovery_context() {
        let node = || NodeName::new("worker").assert_value();
        let cases = [
            (
                NativeV2CliError::InvalidRun(NativeV2AdmissionError::MissingRuntimeBinding {
                    node: node(),
                }),
                "runtime.missing_binding",
                Some("worker"),
                json!({}),
            ),
            (
                NativeV2CliError::InvalidRun(NativeV2AdmissionError::MergePlanAgentGitHubToken {
                    node: node(),
                }),
                "runtime.invalid_environment",
                Some("worker"),
                json!({"environment": GITHUB_TOKEN_ENV}),
            ),
            (
                NativeV2CliError::InvalidRun(NativeV2AdmissionError::DeliveryNodeCount {
                    policy: crate::native_v2_admission::DeliveryPolicy::Optional,
                    found: 2,
                }),
                "request.invalid",
                None,
                json!({"found": 2}),
            ),
            (
                NativeV2CliError::RunEnvironment(RunEnvironmentError::MissingConnection(
                    ConnectionKey::new("anthropic").assert_value(),
                )),
                "runtime.missing_environment",
                None,
                json!({
                    "connection": "anthropic",
                    "helpCommand": CONNECTION_SET_HELP_COMMAND,
                }),
            ),
            (
                NativeV2CliError::RunEnvironment(RunEnvironmentError::MissingField(
                    ConnectionKey::new("openrouter").assert_value(),
                    EnvironmentVariableName::new("OPENROUTER_API_KEY").assert_value(),
                )),
                "runtime.missing_environment",
                None,
                json!({
                    "connection": "openrouter",
                    "requiredFields": ["OPENROUTER_API_KEY"],
                    "helpCommand": CONNECTION_SET_HELP_COMMAND,
                }),
            ),
            (
                NativeV2CliError::RunEnvironment(RunEnvironmentError::InvalidPlan),
                "runtime.invalid_environment",
                None,
                json!({}),
            ),
        ];

        for (error, code, expected_node, expected_details) in cases {
            let value = serde_json::to_value(error.diagnostic()).assert_value();
            assert_eq!(value.assert_key("schema"), ERROR_SCHEMA);
            assert_eq!(value.assert_key("kind"), "invalid_request");
            assert_eq!(value.assert_key("code"), code);
            let expected_node = expected_node.map_or(Value::Null, |node| json!(node));
            assert_eq!(value.assert_key("node"), &expected_node);
            assert_eq!(value.assert_key("details"), &expected_details);
        }
    }

    #[test]
    fn parser_diagnostics_classify_clap_and_json_failures() {
        for (message, code) in [
            ("invalid value for --template", "template.unknown"),
            ("invalid value for <TEMPLATE>", "template.unknown"),
            (
                "template delivery mode is unavailable",
                "template.unsupported_delivery",
            ),
            (
                "merge is not supported by --template research",
                "template.unsupported_delivery",
            ),
            ("another parser failure", "request.invalid"),
        ] {
            let value =
                serde_json::to_value(NativeV2CliError::Usage(message.to_owned()).diagnostic())
                    .assert_value();
            assert_eq!(value.assert_key("code"), code);
        }

        #[derive(Debug, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct StrictJson {
            _known: bool,
        }
        for (source, code) in [
            (
                serde_json::from_str::<StrictJson>(r#"{"unexpected":true}"#)
                    .expect_err("unknown field must be rejected"),
                "json.unknown_field",
            ),
            (
                serde_json::from_str::<Value>("{").expect_err("truncated JSON must be rejected"),
                "json.invalid",
            ),
        ] {
            let value = serde_json::to_value(
                NativeV2CliError::Json {
                    kind: "graph",
                    path: std::path::PathBuf::from("graph.json"),
                    source,
                }
                .diagnostic(),
            )
            .assert_value();
            assert_eq!(value.assert_key("code"), code);
            let details = value.assert_key("details");
            assert_eq!(details.assert_key("fileKind"), "graph");
            assert_eq!(details.assert_key("filePath"), "graph.json");
            assert!(details.assert_key("line").is_number());
            assert!(details.assert_key("column").is_number());
        }
    }

    #[test]
    fn run_and_remote_diagnostics_normalize_kind_and_detail_shape() {
        let cases = [
            (
                NativeV2CliError::RunNotFound {
                    run_id: "run-local".to_owned(),
                },
                "run_not_found",
                "run.not_found",
                json!({"runId": "run-local"}),
            ),
            (
                NativeV2CliError::SubmissionConflict {
                    existing_run_id: "run-existing".to_owned(),
                },
                "submission_conflict",
                "submission.conflict",
                json!({"existingRunId": "run-existing"}),
            ),
            (
                NativeV2CliError::Remote {
                    code: NOT_FOUND.to_owned(),
                    message: "gone".to_owned(),
                    details: Some(json!({"runId": "run-remote"})),
                },
                "run_not_found",
                "run.not_found",
                json!({"runId": "run-remote"}),
            ),
            (
                NativeV2CliError::Remote {
                    code: RUN_CONFLICT.to_owned(),
                    message: "conflict".to_owned(),
                    details: None,
                },
                "submission_conflict",
                "submission.conflict",
                json!({}),
            ),
            (
                NativeV2CliError::Remote {
                    code: IDEMPOTENCY_REUSE.to_owned(),
                    message: "submission key reused".to_owned(),
                    details: Some(json!({"existingRunId": "run-reused"})),
                },
                "submission_conflict",
                "submission.conflict",
                json!({"existingRunId": "run-reused"}),
            ),
            (
                NativeV2CliError::Remote {
                    code: "CONNECTION_UNAVAILABLE".to_owned(),
                    message: "missing connection".to_owned(),
                    details: None,
                },
                "target",
                "connection_unavailable",
                json!({"helpCommand": CONNECTION_SET_HELP_COMMAND}),
            ),
            (
                NativeV2CliError::Remote {
                    code: "CUSTOM_FAILURE".to_owned(),
                    message: "opaque details".to_owned(),
                    details: Some(json!(["provider-owned", 3])),
                },
                "target",
                "custom_failure",
                json!({"nativeDetails": ["provider-owned", 3]}),
            ),
        ];

        assert_diagnostic_cases(cases);

        let explicit_help = remote_diagnostic(
            "connection_unavailable",
            "missing connection",
            Some(json!({"helpCommand": "custom recovery"})),
        );
        assert_eq!(
            serde_json::to_value(explicit_help)
                .assert_value()
                .assert_key("details")
                .assert_key("helpCommand"),
            "custom recovery"
        );
    }

    #[test]
    fn protocol_and_fallback_diagnostics_remain_distinct() {
        let cases = [
            (
                NativeV2CliError::InitialInput("expected an object".to_owned()),
                "invalid_request",
                "input.type_mismatch",
                json!({"reason": "expected an object"}),
            ),
            (
                NativeV2CliError::InvalidRun(NativeV2AdmissionError::UnsupportedGraphProfile),
                "invalid_request",
                "graph.unsupported_profile",
                json!({}),
            ),
            (
                NativeV2CliError::Environment(
                    EnvironmentVariableName::new("PROVIDER_TOKEN").assert_value(),
                ),
                "invalid_request",
                "runtime.missing_environment",
                json!({"environment": "PROVIDER_TOKEN"}),
            ),
            (
                NativeV2CliError::GitHubToken,
                "invalid_request",
                "runtime.invalid_environment",
                json!({}),
            ),
            (
                NativeV2CliError::Update("checksum mismatch".to_owned()),
                "invalid_request",
                "update.failed",
                json!({}),
            ),
            (
                NativeV2CliError::TargetTransport {
                    name: "cloud".to_owned(),
                    origin: "https://api.example.test".to_owned(),
                    message: "offline".to_owned(),
                },
                "target",
                "target.unavailable",
                json!({"target": "cloud", "origin": "https://api.example.test"}),
            ),
            (
                NativeV2CliError::Protocol("invalid frame".to_owned()),
                "protocol",
                "protocol.invalid",
                json!({}),
            ),
            (
                NativeV2CliError::OutputJson(
                    serde_json::from_str::<Value>("{")
                        .expect_err("truncated JSON must be rejected"),
                ),
                "protocol",
                "protocol.invalid",
                json!({}),
            ),
            (
                NativeV2CliError::Local("profile store unavailable".to_owned()),
                "target",
                "target.unavailable",
                json!({}),
            ),
        ];

        assert_diagnostic_cases(cases);
    }

    #[test]
    fn client_errors_preserve_typed_failures_and_fail_closed_without_them() {
        let backend = client_error(ClientError::Backend(BackendError::invalid_params(
            "BAD_REQUEST",
            "invalid request",
            Some(json!({"field": "graph"})),
        )));
        assert!(matches!(
            backend,
            NativeV2CliError::Remote { code, details: Some(details), .. }
                if code == "BAD_REQUEST" && details == json!({"field": "graph"})
        ));

        let rpc = |data| {
            ClientError::Rpc(JsonRpcError {
                code: -32000,
                message: "rpc failure".to_owned(),
                data,
            })
        };
        let typed = client_error(rpc(Some(DomainErrorData {
            code: "TYPED".to_owned(),
            details: Some(json!({"retry": false})),
        })));
        assert!(matches!(
            typed,
            NativeV2CliError::Remote { code, details: Some(details), .. }
                if code == "TYPED" && details == json!({"retry": false})
        ));
        assert!(matches!(
            client_error(rpc(None)),
            NativeV2CliError::Protocol(message) if message == "rpc failure"
        ));
        assert!(matches!(
            client_error(ClientError::Transport(TransportError::Protocol(
                "closed".to_owned()
            ))),
            NativeV2CliError::Disconnected
        ));
        assert!(matches!(
            client_error(ClientError::InvalidResponse("not JSON-RPC".to_owned())),
            NativeV2CliError::Protocol(message) if message.contains("not JSON-RPC")
        ));
    }
}
