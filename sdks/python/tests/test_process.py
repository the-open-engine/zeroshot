"""Native JSON error contract projections."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from zeroshot import ProtocolError, TargetError
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


@pytest.mark.parametrize("details", [{}, {"runId": None}, {"runId": 42}, {"runId": ""}])
def test_failed_run_diagnostic_requires_a_run_identity(tmp_path: Path, details: object) -> None:
    error = native_process(tmp_path)._error(diagnostic(details=details), 1)

    assert isinstance(error, ProtocolError)


def test_failed_run_kind_requires_the_failed_run_code(tmp_path: Path) -> None:
    error = native_process(tmp_path)._error(
        diagnostic(code="target.unavailable", details={"runId": "failed-run"}), 1
    )

    assert isinstance(error, ProtocolError)
