use crate::assertions::AssertError;
use openengine_cluster_protocol::{
    ClusterStatus, GraphProfile, InitializeResult, Phase, ServerCapabilities,
};
use serde_json::json;

use super::*;

struct WrongAttachBackend;

#[async_trait::async_trait]
impl ClusterBackend for WrongAttachBackend {
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: openengine_cluster_protocol::InitializeParams,
    ) -> Result<InitializeResult, openengine_cluster_server::BackendError> {
        unreachable!("agent attach probe does not initialize")
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        _params: openengine_cluster_protocol::GetParams,
    ) -> Result<GetResult, openengine_cluster_server::BackendError> {
        unreachable!("agent attach probe does not get")
    }

    async fn agent_attach(
        &self,
        _context: &ConnectionContext,
        _params: AgentAttachParams,
        _queue_capacity: usize,
    ) -> Result<
        (
            openengine_cluster_protocol::AgentAttachResult,
            openengine_cluster_server::agent_attach::AgentAttachEventStream,
            openengine_cluster_server::agent_attach::AgentAttachHandle,
        ),
        openengine_cluster_server::BackendError,
    > {
        Err(openengine_cluster_server::BackendError::application(
            "WRONG_ATTACH_ERROR",
            "wrong attach error",
            None,
        ))
    }
}

fn registration(
    graph_profiles: &'static [GraphProfile],
    optional: RegisteredOptionalCapabilities,
) -> BackendRegistration<'static> {
    BackendRegistration {
        graph_profiles,
        optional,
    }
}

fn success(result: impl serde::Serialize) -> Value {
    json!({"jsonrpc":"2.0", "id":1, "result":result})
}

#[test]
fn response_validation_rejects_envelope_and_direct_expectation_mismatches() {
    let empty = registration(&[], RegisteredOptionalCapabilities::default());
    assert_eq!(
        validate_response(Expected::EmptyGet, &json!({"jsonrpc":"1.0"}), empty),
        Err("response jsonrpc was not 2.0".to_owned())
    );
    for expected in [
        Expected::WatchEstablished,
        Expected::LogsEstablished,
        Expected::AgentAttachNotFound,
    ] {
        assert_eq!(
            validate_response(expected, &json!({"jsonrpc":"2.0"}), empty),
            Err("direct expectation reached JSON-RPC validator".to_owned())
        );
    }
}

#[test]
fn initialize_validation_distinguishes_shape_registration_and_capability_failures() {
    let empty = registration(&[], RegisteredOptionalCapabilities::default());
    assert!(
        validate_initialize(&json!({"jsonrpc":"2.0", "id":1}), empty)
            .assert_error()
            .starts_with("initialize failed:")
    );
    assert!(
        validate_initialize(&success(json!({})), empty)
            .assert_error()
            .starts_with("invalid initialize result:")
    );

    let initialized = InitializeResult::new(ServerCapabilities::default(), ClusterStatus::empty());
    assert!(validate_initialize(&success(&initialized), empty).is_ok());
    assert!(
        validate_initialize(
            &success(&initialized),
            registration(
                &[GraphProfile::Full],
                RegisteredOptionalCapabilities::default(),
            ),
        )
        .assert_error()
        .contains("did not match registration")
    );
    assert_eq!(
        validate_initialize(
            &success(initialized),
            registration(
                &[],
                RegisteredOptionalCapabilities {
                    logs: true,
                    agent_attach: false,
                },
            ),
        ),
        Err("advertised optional capabilities did not match factory registration".to_owned())
    );
}

#[test]
fn get_and_error_validation_preserve_specific_failure_diagnostics() {
    assert!(
        validate_empty_get(&json!({"jsonrpc":"2.0", "id":1}))
            .assert_error()
            .starts_with("get failed:")
    );
    assert!(
        validate_empty_get(&success(json!({})))
            .assert_error()
            .starts_with("invalid get result:")
    );
    let nonempty = GetResult {
        status: ClusterStatus {
            phase: Phase::Running,
            ..ClusterStatus::empty()
        },
        ..GetResult::empty()
    };
    assert!(
        validate_empty_get(&success(nonempty))
            .assert_error()
            .starts_with("fresh get was not canonical empty:")
    );

    for (response, expected) in [
        (
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-1}}),
            "expected error code -32602",
        ),
        (
            json!({"jsonrpc":"2.0","id":2,"error":{"code":-32602}}),
            "expected response id 1",
        ),
        (
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32602,"data":{"code":"OTHER"}}}),
            "expected domain code SCHEMA_VIOLATION",
        ),
    ] {
        let error = validate_error_response(&response, -32602, Some("SCHEMA_VIOLATION"), Some(1))
            .assert_error();
        assert!(error.starts_with(expected), "{error}");
    }
}

#[test]
fn failure_report_accessors_and_rendering_preserve_case_context() {
    let failure = CaseFailure {
        id: "portable.case.failed",
        message: "specific refusal".to_owned(),
    };
    assert_eq!(failure.id(), "portable.case.failed");
    assert_eq!(failure.message(), "specific refusal");
    let failures = ConformanceFailures {
        cases: vec![
            CaseResult {
                id: "portable.case.passed",
                disposition: CaseDisposition::Passed,
            },
            CaseResult {
                id: "portable.case.skipped",
                disposition: CaseDisposition::Skipped(OptionalCapability::Logs),
            },
        ],
        failures: vec![failure],
    };
    assert_eq!(failures.passed(), 1);
    assert_eq!(failures.skipped(), 1);
    assert_eq!(
        failures.to_string(),
        "1 portable backend conformance case(s) failed\nportable.case.failed: specific refusal\n"
    );
}

#[tokio::test]
async fn agent_attach_probe_reports_the_actual_unexpected_backend_code() {
    let dispatcher = Dispatcher::new(WrongAttachBackend, ConnectionContext::default());
    assert_eq!(
        exercise_agent_attach(&dispatcher).await,
        Err(format!(
            "agent attach returned WRONG_ATTACH_ERROR, expected {NOT_FOUND}"
        ))
    );
}
