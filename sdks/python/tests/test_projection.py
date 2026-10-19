"""Strict native projection tests."""

from __future__ import annotations

import pytest

from zeroshot._projection import _log_event, _plan_status, _status, _summary
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


def local_status() -> dict[str, object]:
    return {
        "runId": "folder-run",
        "title": "Ordinary folder",
        "size": "small",
        "atCursor": "v2:0",
        "status": {"phase": "admitted"},
    }


@pytest.mark.parametrize("explicit_null", [False, True])
def test_local_status_and_inventory_preserve_source_absence(explicit_null: bool) -> None:
    value = local_status()
    if explicit_null:
        value["source"] = None
    status = _status(value)
    assert status.source is None
    assert _summary(status).source is None


def test_generic_git_source_preserves_remote_and_opaque_forge_identity() -> None:
    value = local_status()
    value["source"] = {
        "repository": "nested/team/project:42",
        "branch": "main",
        "revision": "a" * 64,
        "remote": {
            "url": "ssh://alice@git.example:2222/team/nested/project.git",
            "connection": "source-private",
            "forge": {"provider": "source.contributed", "repository": "opaque:project"},
        },
    }
    source = _status(value).source
    assert source is not None
    assert source.repository == "nested/team/project:42"
    assert source.revision == "a" * 64
    assert source.remote is not None
    assert source.remote.url == "ssh://alice@git.example:2222/team/nested/project.git"
    assert source.remote.connection == "source-private"
    assert source.remote.forge is not None
    assert source.remote.forge.provider == "source.contributed"
    assert source.remote.forge.repository == "opaque:project"


@pytest.mark.parametrize("source", [False, "repo", {"repository": "owner/repo"}])
def test_present_malformed_source_remains_a_protocol_error(source: object) -> None:
    value = local_status()
    value["source"] = source
    with pytest.raises(ProtocolError, match="source"):
        _status(value)


def failed_status(context: object = None) -> dict[str, object]:
    value = local_status()
    terminal = {"status": "failed", "reason": "delivery_failed"}
    if context is not None:
        terminal["context"] = context
    value["status"] = {"phase": "finished", "terminalResult": terminal}
    return value


def test_failed_status_retains_immutable_structured_evidence_without_inventing_a_cause() -> None:
    from dataclasses import FrozenInstanceError

    from zeroshot import ActiveExecution, FailureContext, FailureDiagnostic

    context = {
        "primary": {"message": "Runtime cleanup failed"},
        "recorded": [
            {
                "message": "GitHub API request failed: HTTP 404 — base branch not found",
                "execution": {"execution": "opaque:execution:1", "node": "deliver"},
                "cursor": "v2:3",
            }
        ],
        "incomplete": True,
    }
    result = _status(failed_status(context)).result
    assert result is not None
    assert result.failure == "delivery_failed"
    assert result.context == FailureContext(
        primary=FailureDiagnostic(message="Runtime cleanup failed"),
        recorded=(
            FailureDiagnostic(
                message="GitHub API request failed: HTTP 404 — base branch not found",
                execution=ActiveExecution(execution="opaque:execution:1", node="deliver"),
                cursor="v2:3",
            ),
        ),
        incomplete=True,
    )
    context["recorded"].clear()
    assert len(result.context.recorded) == 1
    with pytest.raises(FrozenInstanceError):
        result.context.recorded[0].message = "changed"


def test_old_failed_status_has_no_inferred_context() -> None:
    result = _status(failed_status()).result
    assert result is not None
    assert result.failure == "delivery_failed"
    assert result.context is None


@pytest.mark.parametrize(
    "context",
    [
        False,
        "failure",
        {"incomplete": "true"},
        {"recorded": None},
        {"recorded": [{"message": 123}]},
        {"primary": {"message": "failed", "execution": "1"}},
        {"primary": {"message": "failed", "cursor": 3}},
    ],
)
def test_failed_status_rejects_malformed_failure_evidence(context: object) -> None:
    with pytest.raises(ProtocolError, match="failure"):
        _status(failed_status(context))
