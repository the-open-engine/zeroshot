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
        let value = serde_json::to_value(NativeV2CliError::Usage(message.to_owned()).diagnostic())
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
                serde_json::from_str::<Value>("{").expect_err("truncated JSON must be rejected"),
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
