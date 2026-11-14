"""Native JSON error contract projections."""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path

import pytest

from zeroshot import (
    InvalidRequestError,
    ProtocolError,
    RunNotFoundError,
    SubmissionConflictError,
    TargetError,
)
from zeroshot._process import _MAX_STREAM_FRAME_BYTES, NativeProcess, _RunFailed


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


@pytest.mark.parametrize("extra", [0, 1])
def test_request_stream_frame_byte_ceiling(
    fake_native: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, extra: int
) -> None:
    monkeypatch.setenv("FAKE_WATCH_FRAME_BYTES", str(_MAX_STREAM_FRAME_BYTES + extra))
    process = NativeProcess(
        Path(os.environ["ZEROSHOT_PYTHON_NATIVE_BINARY"]),
        cwd=tmp_path,
        environment=os.environ,
        secrets=(),
    )

    async def exercise() -> None:
        if extra:
            with pytest.raises(ProtocolError, match="oversized NDJSON"):
                async for _ in process.json_lines(["watch", "original"]):
                    pass
        else:
            events = [event async for event in process.json_lines(["watch", "original"])]
            assert len(events) == 2
            assert events[-1]["padding"].startswith("x")

    asyncio.run(exercise())


def test_oversized_request_stream_is_a_protocol_error_and_stops_the_child(
    fake_native: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    pid_path = tmp_path / "watch.pid"
    monkeypatch.setenv("FAKE_REQUEST_STREAM_BYTES", str(_MAX_STREAM_FRAME_BYTES + 1))
    monkeypatch.setenv("FAKE_WATCH_HOLD_OPEN", "60")
    monkeypatch.setenv("FAKE_WATCH_PID", str(pid_path))
    process = NativeProcess(
        Path(os.environ["ZEROSHOT_PYTHON_NATIVE_BINARY"]),
        cwd=tmp_path,
        environment=os.environ,
        secrets=(),
    )

    async def exercise() -> None:
        with pytest.raises(ProtocolError, match="oversized NDJSON"):
            async for _ in process.json_lines(["watch", "original"]):
                pass

    asyncio.run(asyncio.wait_for(exercise(), timeout=10))
    if os.name != "nt":
        with pytest.raises(ProcessLookupError):
            os.kill(int(pid_path.read_text()), 0)
