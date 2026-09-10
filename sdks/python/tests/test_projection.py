"""Strict native projection tests."""

from __future__ import annotations

import pytest

from zeroshot._projection import _log_event, _plan_status
from zeroshot.errors import ProtocolError


def log_event(*, timestamp: object = 1_725_000_000_123) -> dict[str, object]:
    return {
        "runId": "run-1",
        "cursor": "v2:3",
        "timestamp": timestamp,
        "record": {"level": "info", "target": "agent", "message": "done"},
    }


@pytest.mark.parametrize("timestamp", [1, 1_725_000_000_123, 9_007_199_254_740_991])
def test_log_event_preserves_exact_timestamp(timestamp: int) -> None:
    assert _log_event(log_event(timestamp=timestamp)).timestamp == timestamp


@pytest.mark.parametrize(
    "timestamp",
    [True, 1.0, "1", 0, -1, 9_007_199_254_740_992],
)
def test_log_event_rejects_invalid_timestamp(timestamp: object) -> None:
    with pytest.raises(ProtocolError, match=r"malformed log event\.timestamp"):
        _log_event(log_event(timestamp=timestamp))


def test_log_event_rejects_missing_timestamp() -> None:
    value = log_event()
    del value["timestamp"]
    with pytest.raises(ProtocolError, match=r"malformed log event\.timestamp"):
        _log_event(value)


def plan_status() -> dict[str, object]:
    return {
        "planId": "plan-1",
        "title": "Release",
        "state": "queued",
        "repository": "owner/repo",
        "branch": "main",
        "submittedAt": "2026-09-10T00:00:00Z",
        "expiresAt": "2026-09-11T00:00:00Z",
        "runs": [
            {
                "name": "build",
                "runId": "run-1",
                "state": "blocked",
                "needs": [],
                "sourceRevision": None,
                "readyAt": None,
                "queueExpiresAt": None,
                "terminalAt": None,
                "waitingReason": "dependencies_pending",
                "errorCode": None,
            }
        ],
    }


def test_merge_plan_projection_preserves_explicit_nullable_fields() -> None:
    status = _plan_status(plan_status())
    assert status.plan_id == "plan-1"
    assert status.runs[0].source_revision is None
    assert status.runs[0].waiting_reason == "dependencies_pending"
    assert not status.terminal


def test_merge_plan_projection_rejects_omitted_nullable_fields() -> None:
    value = plan_status()
    runs = value["runs"]
    assert isinstance(runs, list)
    del runs[0]["queueExpiresAt"]
    with pytest.raises(ProtocolError, match="queueExpiresAt"):
        _plan_status(value)
