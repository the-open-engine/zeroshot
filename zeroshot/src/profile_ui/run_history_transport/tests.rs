use openengine_cluster_protocol::{Cursor, RunId};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

#[test]
fn requests_select_their_exact_response_budget() {
    let id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c992");
    let requests = [
        (RunHistoryRequest::list(None), RUN_HISTORY_LIST_MAX_BYTES),
        (
            RunHistoryRequest::detail(id.clone()),
            RUN_HISTORY_DEFINITION_MAX_BYTES,
        ),
        (
            RunHistoryRequest::page(id, Cursor::new("v2:41")),
            RUN_HISTORY_PAGE_MAX_BYTES,
        ),
    ];
    for (request, expected) in requests {
        assert_eq!(request.maximum_response_bytes(), expected);
        assert_eq!(
            request.maximum_problem_bytes(),
            RUN_HISTORY_PROBLEM_MAX_BYTES
        );
    }
}

#[test]
fn response_status_is_bounded_and_transport_errors_are_sanitized() {
    for invalid in [0, 99, 600, u16::MAX] {
        assert_eq!(
            RunHistoryResponse::new(invalid, Vec::new()).assert_error(),
            RunHistoryTransportError::Incompatible
        );
    }
    for valid in [100, 200, 404, 599] {
        let body = vec![valid as u8];
        assert_eq!(
            RunHistoryResponse::new(valid, body.clone())
                .assert_value()
                .into_parts(),
            (valid, body)
        );
    }
    assert_eq!(
        RunHistoryTransportError::incompatible().to_string(),
        "run history capability is incompatible"
    );
    assert_eq!(
        RunHistoryTransportError::unavailable().to_string(),
        "run history transport is unavailable"
    );
}
