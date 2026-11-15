"""Strict native projection tests."""

from __future__ import annotations

import pytest

from zeroshot._projection import _log_event, _stack_status, _status, _summary
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


def stack_status() -> dict[str, object]:
    return {
        "stackId": "stack-1",
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
                "sourceBranch": None,
                "sourceRevision": None,
                "readyAt": None,
                "queueExpiresAt": None,
                "terminalAt": None,
                "waitingReason": "dependencies_pending",
                "errorCode": None,
            }
        ],
    }


def test_stack_projection_preserves_explicit_nullable_fields() -> None:
    status = _stack_status(stack_status())
    assert status.stack_id == "stack-1"
    assert status.runs[0].source_branch is None
    assert status.runs[0].source_revision is None
    assert status.runs[0].waiting_reason == "dependencies_pending"
    assert not status.terminal


def test_stack_projection_preserves_the_stacked_source_branch() -> None:
    value = stack_status()
    runs = value["runs"]
    assert isinstance(runs, list)
    runs[0]["sourceBranch"] = "zeroshot/v2-0123456789abcdef0123"
    status = _stack_status(value)
    assert status.runs[0].source_branch == "zeroshot/v2-0123456789abcdef0123"


@pytest.mark.parametrize("field", ["sourceBranch", "queueExpiresAt"])
def test_stack_projection_rejects_omitted_nullable_fields(field: str) -> None:
    value = stack_status()
    runs = value["runs"]
    assert isinstance(runs, list)
    del runs[0][field]
    with pytest.raises(ProtocolError, match=field):
        _stack_status(value)


def local_status() -> dict[str, object]:
    return {
        "runId": "folder-run",
        "title": "Ordinary folder",
        "size": "small",
        "atCursor": "v2:0",
        "status": {"phase": "admitted"},
    }


@pytest.mark.parametrize("phase", ["running", "stopping"])
@pytest.mark.parametrize("succeeded", [False, True])
def test_graph_result_is_visible_without_becoming_a_terminal_result(
    phase: str, succeeded: bool
) -> None:
    value = local_status()
    graph = (
        {"status": "succeeded", "output": {"answer": 42}}
        if succeeded
        else {"status": "failed", "reason": "worker_failed", "context": {"incomplete": True}}
    )
    value["status"] = {"phase": phase, "activeExecutions": [], "graphResult": graph}
    status = _status(value)
    assert status.phase == phase
    assert status.result is None
    assert status.graph_result is not None
    assert status.graph_result.run_id == "folder-run"
    assert status.graph_result.succeeded == succeeded
    assert status.graph_result.output == ({"answer": 42} if succeeded else None)
    assert status.graph_result.failure == (None if succeeded else "worker_failed")
    if not succeeded:
        assert status.graph_result.context is not None
        assert status.graph_result.context.incomplete
    assert _summary(status).graph_result == status.graph_result


@pytest.mark.parametrize("phase", ["admitted", "running", "stopping", "finished"])
def test_absent_graph_result_stays_absent(phase: str) -> None:
    value = local_status()
    value["status"] = (
        {"phase": phase, "terminalResult": {"status": "succeeded", "output": None}}
        if phase == "finished"
        else {"phase": phase}
    )
    assert _status(value).graph_result is None
    assert _summary(_status(value)).graph_result is None


@pytest.mark.parametrize("graph", [False, {"status": "failed"}, {"status": "succeeded"}])
def test_malformed_graph_result_is_rejected(graph: object) -> None:
    value = local_status()
    value["status"] = {"phase": "running", "activeExecutions": [], "graphResult": graph}
    with pytest.raises(ProtocolError):
        _status(value)


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


@pytest.mark.parametrize("size", ["medium", "custom-big-box", "GPU/2027", "tiny", "standard"])
def test_machine_sizes_survive_status_and_summary_projection(size: str) -> None:
    value = local_status()
    value["size"] = size
    status = _status(value)
    assert status.size == size
    assert _summary(status).size == size


@pytest.mark.parametrize("size", [None, 3, ["medium"]])
def test_machine_size_projection_rejects_non_string_values(size: object) -> None:
    value = local_status()
    value["size"] = size
    with pytest.raises(ProtocolError, match=r"run status\.size"):
        _status(value)


@pytest.mark.parametrize("failed", [False, True])
def test_terminal_receipt_is_independent_of_graph_output(failed: bool) -> None:
    value = local_status()
    terminal = (
        {"status": "failed", "reason": "rejected"}
        if failed
        else {"status": "succeeded", "output": {"summary": "authored report"}}
    )
    receipt = {"receipt": {"mode": "comment", "outcome": "posted"}}
    value["status"] = {
        "phase": "finished",
        "terminalResult": terminal,
        "metadata": {"deliveryReceipt": receipt},
    }
    result = _status(value).result
    assert result is not None
    assert result.succeeded is not failed
    assert result.delivery_receipt == receipt
    assert result.output == (None if failed else {"summary": "authored report"})


def test_graph_output_cannot_supply_a_delivery_receipt() -> None:
    value = local_status()
    value["status"] = {
        "phase": "finished",
        "terminalResult": {"status": "succeeded", "output": {"mode": "merge", "outcome": "merged"}},
    }
    result = _status(value).result
    assert result is not None
    assert result.delivery_receipt is None


@pytest.mark.parametrize(
    "message",
    [
        "Node writer failed: refusal: authentication required",
        "Node writer failed: refusal: model unavailable: model missing-model",
        "Node writer was stopped after 1.0s",
    ],
)
def test_provider_rejection_and_stopped_diagnostics_preserve_native_messages(message: str) -> None:
    from zeroshot._projection import _failure_context

    context = _failure_context({"recorded": [{"message": message}]})
    assert context is not None
    assert context.recorded[0].message == message
    wire = log_event()
    wire["record"] = {"level": "error", "target": "node", "message": message}
    assert _log_event(wire).message == message


@pytest.mark.parametrize("state", ["lost", "unknown"])
def test_controller_observation_stays_separate_from_recorded_status(state: str) -> None:
    value = local_status()
    value["controller"] = {"state": state, "observedAt": 1_794_470_707_000}
    status = _status(value)
    assert status.controller is not None
    assert status.controller.state == state
    assert status.controller.observed_at == 1_794_470_707_000
    assert status.result is None
    assert _summary(status).controller == status.controller
    assert _status(local_status()).controller is None


@pytest.mark.parametrize(
    "observation",
    [
        {"state": "dead", "observedAt": 1},
        {"state": "lost"},
        {"state": "unknown", "observedAt": 0},
        {"state": "lost", "observedAt": True},
        {"state": "lost", "observedAt": 9_007_199_254_740_992},
    ],
)
def test_invalid_controller_observation_is_rejected(observation: object) -> None:
    value = local_status()
    value["controller"] = observation
    with pytest.raises(ProtocolError):
        _status(value)


@pytest.mark.parametrize("reason", ["command", "wait"])
def test_suspended_status_and_summary_preserve_reason_node_and_times(reason: str) -> None:
    value = local_status()
    status: dict[str, object] = {
        "phase": "suspended",
        "reason": reason,
        "node": "cooldown",
        "createdAt": 1762905600000,
    }
    if reason == "wait":
        status["wait"] = {"dueAt": 1762992000000}
    value["status"] = status
    projected = _status(value)
    assert projected.phase == "suspended"
    assert projected.reason == reason
    assert projected.node == "cooldown"
    assert projected.created_at == 1762905600000
    assert projected.due_at == (1762992000000 if reason == "wait" else None)
    assert _summary(projected).reason == reason
    assert projected.result is None


def test_running_status_exposes_requested_suspension() -> None:
    value = local_status()
    value["status"] = {"phase": "running", "activeExecutions": [], "suspendRequested": True}
    assert _status(value).suspend_requested
    assert _summary(_status(value)).suspend_requested


@pytest.mark.parametrize("reason,wait", [("wait", None), ("command", {"dueAt": 1762992000000})])
def test_suspended_status_rejects_incoherent_due_time(reason: str, wait: object) -> None:
    value = local_status()
    value["status"] = {
        "phase": "suspended",
        "reason": reason,
        "node": "cooldown",
        "createdAt": 1762905600000,
        "wait": wait,
    }
    with pytest.raises(ProtocolError, match=r"suspension\.wait"):
        _status(value)
