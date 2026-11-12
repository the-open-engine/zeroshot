"""Native JSON error contract projections."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from zeroshot import (
    InvalidRequestError,
    ProtocolError,
    RunNotFoundError,
    SubmissionConflictError,
    TargetError,
)
from zeroshot._process import NativeProcess, _RunFailed


def diagnostic(*, code: str = "run.failed", details: object) -> bytes:
    return json.dumps(
        {
            "schema": "zeroshot.error/v1",
            "kind": "run_failed",
            "code": code,
            "message": "run failed: provider-secret",
            "path": None,
            "node": None,
            "details": details,
        }
    ).encode()


def native_process(tmp_path: Path) -> NativeProcess:
    return NativeProcess(
        Path("zeroshot"), cwd=tmp_path, environment={}, secrets=("provider-secret",)
    )


def test_failed_run_diagnostic_preserves_identity_and_redacts_message(tmp_path: Path) -> None:
    error = native_process(tmp_path)._error(diagnostic(details={"runId": "failed-run"}), 1)

    assert isinstance(error, _RunFailed)
    assert not isinstance(error, TargetError)
    assert error.run_id == "failed-run"
    assert str(error) == "run failed: <redacted>"


@pytest.mark.parametrize("secret", ["1", "provider-secret-1", "run.failed"])
def test_failed_run_identity_is_not_redacted(tmp_path: Path, secret: str) -> None:
    run_id = "provider-secret-1"
    native = NativeProcess(
        Path("zeroshot"), cwd=tmp_path, environment={}, secrets=("provider-secret", secret)
    )

    error = native._error(diagnostic(details={"runId": run_id}), 1)

    assert isinstance(error, _RunFailed)
    assert error.run_id == run_id
    assert str(error) == "run failed: <redacted>"
    assert run_id not in repr(error)


@pytest.mark.parametrize("details", [{}, {"runId": None}, {"runId": 42}, {"runId": ""}])
def test_failed_run_diagnostic_requires_a_run_identity(tmp_path: Path, details: object) -> None:
    error = native_process(tmp_path)._error(diagnostic(details=details), 1)

    assert isinstance(error, ProtocolError)


def test_failed_run_kind_requires_the_failed_run_code(tmp_path: Path) -> None:
    error = native_process(tmp_path)._error(
        diagnostic(code="target.unavailable", details={"runId": "failed-run"}), 1
    )

    assert isinstance(error, ProtocolError)


@pytest.mark.parametrize(
    ("kind", "code", "details", "expected"),
    [
        ("run_failed", "run.failed", {"runId": "failed-run"}, _RunFailed),
        ("target", "target.unavailable", {}, TargetError),
        ("invalid_request", "request.invalid", {}, InvalidRequestError),
        ("protocol", "protocol.invalid", {}, ProtocolError),
        ("run_not_found", "run.not_found", {}, RunNotFoundError),
        (
            "submission_conflict",
            "submission.conflict",
            {"existingRunId": "other"},
            SubmissionConflictError,
        ),
        ("run_failed", "run.failed", {}, ProtocolError),
    ],
)
def test_notice_before_terminal_diagnostic_keeps_error_type(
    tmp_path: Path, kind: str, code: str, details: dict[str, str], expected: type[Exception]
) -> None:
    value = json.loads(diagnostic(code=code, details=details))
    value["kind"] = kind
    stderr = (
        b'Connection to target "prod" lost; the run continues there.\n' + json.dumps(value).encode()
    )
    error = native_process(tmp_path)._error(stderr, 1)
    assert isinstance(error, expected)
    assert "provider-secret" not in str(error)


def test_terminal_line_takes_precedence_over_earlier_diagnostic(tmp_path: Path) -> None:
    earlier = diagnostic(details={"runId": "failed-run"})
    error = native_process(tmp_path)._error(earlier + b"\ntransport failed: provider-secret", 1)
    assert type(error) is TargetError
    assert "transport failed: <redacted>" in str(error)


def test_complete_multiline_diagnostic_remains_supported(tmp_path: Path) -> None:
    stderr = json.dumps(json.loads(diagnostic(details={"runId": "failed-run"})), indent=2).encode()
    error = native_process(tmp_path)._error(stderr, 1)
    assert isinstance(error, _RunFailed)
    assert error.run_id == "failed-run"
