"""Strict projections from native JSON into public immutable read models."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Literal, TypeAlias, cast

from .errors import ProtocolError
from .plans import MergePlanRunStatus, MergePlanStatus
from .runs import ActiveExecution, LogEvent, ResolvedSource, RunResult, RunStatus, RunSummary
from .values import JsonValue

_Size: TypeAlias = Literal["small", "medium", "large"]
_Phase: TypeAlias = Literal["admitted", "running", "stopping", "finished"]
_Level: TypeAlias = Literal["debug", "info", "error"]
_MAX_JAVASCRIPT_SAFE_INTEGER = 9_007_199_254_740_991


def _status(value: object) -> RunStatus:
    root = _mapping(value, "run status")
    native_status = _mapping(root.get("status"), "run status.status")
    phase = cast(
        _Phase,
        _enum_string(
            native_status,
            "phase",
            {"admitted", "running", "stopping", "finished"},
            "run status.status",
        ),
    )
    active = tuple(
        ActiveExecution(
            execution=_string(item, "execution", "active execution"),
            node=_string(item, "node", "active execution"),
        )
        for item in _mapping_list(
            native_status.get("activeExecutions", []),
            "run status.status.activeExecutions",
        )
    )
    result = None
    if phase == "finished":
        result = _terminal_result(
            _string(root, "runId", "run status"),
            native_status.get("terminalResult"),
        )
    return RunStatus(
        run_id=_string(root, "runId", "run status"),
        title=_string(root, "title", "run status"),
        source=_source(root.get("source")),
        size=cast(
            _Size,
            _enum_string(
                root,
                "size",
                {"small", "medium", "large"},
                "run status",
            ),
        ),
        cursor=_cursor(root),
        phase=phase,
        active_executions=active,
        result=result,
    )


def _summary(status: RunStatus) -> RunSummary:
    failure = status.result.failure if status.result is not None else None
    return RunSummary(
        run_id=status.run_id,
        title=status.title,
        source=status.source,
        size=status.size,
        cursor=status.cursor,
        phase=status.phase,
        force_stop_requested=status.phase == "stopping" or failure == "force_stopped",
    )


def _log_event(value: object) -> LogEvent:
    root = _mapping(value, "log event")
    record = _mapping(root.get("record"), "log event.record")
    execution = root.get("execution")
    if execution is not None and not isinstance(execution, str):
        raise ProtocolError("Zeroshot emitted a non-string log execution selector")
    return LogEvent(
        run_id=_string(root, "runId", "log event"),
        cursor=_string(root, "cursor", "log event"),
        timestamp=_positive_javascript_safe_integer(root, "timestamp", "log event"),
        execution=execution,
        level=cast(
            _Level,
            _enum_string(record, "level", {"debug", "info", "error"}, "log event.record"),
        ),
        target=_string(record, "target", "log event.record"),
        message=_string(record, "message", "log event.record"),
    )


def _source(value: object) -> ResolvedSource:
    source = _mapping(value, "run status.source")
    return ResolvedSource(
        repository=_string(source, "repository", "run status.source"),
        branch=_string(source, "branch", "run status.source"),
        revision=_string(source, "revision", "run status.source"),
    )


def _terminal_result(run_id: str, value: object) -> RunResult:
    terminal = _mapping(value, "run status.status.terminalResult")
    kind = _enum_string(
        terminal,
        "status",
        {"succeeded", "failed"},
        "run status.status.terminalResult",
    )
    if kind == "succeeded":
        if "output" not in terminal:
            raise ProtocolError("Zeroshot omitted terminal success output")
        return RunResult(
            run_id=run_id,
            succeeded=True,
            output=cast(JsonValue, terminal["output"]),
        )
    return RunResult(
        run_id=run_id,
        succeeded=False,
        failure=_string(terminal, "reason", "run status.status.terminalResult"),
    )


def _cursor(value: Mapping[str, object]) -> str:
    for name in ("cursor", "atCursor"):
        candidate = value.get(name)
        if isinstance(candidate, str):
            return candidate
    raise ProtocolError("Zeroshot omitted the run status cursor")


def _mapping(value: object, kind: str) -> Mapping[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ProtocolError(f"Zeroshot emitted malformed {kind}")
    return value


def _mapping_list(value: object, kind: str) -> tuple[Mapping[str, object], ...]:
    if not isinstance(value, list):
        raise ProtocolError(f"Zeroshot emitted malformed {kind}")
    return tuple(_mapping(item, kind) for item in value)


def _string(value: Mapping[str, object], name: str, kind: str) -> str:
    selected = value.get(name)
    if not isinstance(selected, str):
        raise ProtocolError(f"Zeroshot emitted malformed {kind}.{name}")
    return selected


def _positive_javascript_safe_integer(
    value: Mapping[str, object],
    name: str,
    kind: str,
) -> int:
    selected = value.get(name)
    if type(selected) is not int or not 1 <= selected <= _MAX_JAVASCRIPT_SAFE_INTEGER:
        raise ProtocolError(f"Zeroshot emitted malformed {kind}.{name}")
    return selected


def _enum_string(
    value: Mapping[str, object],
    name: str,
    allowed: set[str],
    kind: str,
) -> str:
    selected = _string(value, name, kind)
    if selected not in allowed:
        raise ProtocolError(f"Zeroshot emitted unsupported {kind}.{name} {selected!r}")
    return selected


def _plan_status(value: object) -> MergePlanStatus:
    root = _mapping(value, "merge plan status")
    runs = tuple(
        _plan_run_status(item) for item in _mapping_list(root.get("runs"), "merge plan status.runs")
    )
    return MergePlanStatus(
        plan_id=_string(root, "planId", "merge plan status"),
        title=_string(root, "title", "merge plan status"),
        state=cast(
            Literal["queued", "running", "succeeded", "failed", "cancelled", "expired"],
            _enum_string(
                root,
                "state",
                {"queued", "running", "succeeded", "failed", "cancelled", "expired"},
                "merge plan status",
            ),
        ),
        repository=_string(root, "repository", "merge plan status"),
        branch=_string(root, "branch", "merge plan status"),
        submitted_at=_string(root, "submittedAt", "merge plan status"),
        expires_at=_string(root, "expiresAt", "merge plan status"),
        runs=runs,
    )


def _plan_run_status(value: Mapping[str, object]) -> MergePlanRunStatus:
    kind = "merge plan status.runs"
    return MergePlanRunStatus(
        name=_string(value, "name", kind),
        run_id=_string(value, "runId", kind),
        state=cast(
            Literal[
                "blocked",
                "materializing",
                "queued",
                "provisioning",
                "running",
                "cancelling",
                "succeeded",
                "failed",
                "cancelled",
                "expired",
            ],
            _enum_string(
                value,
                "state",
                {
                    "blocked",
                    "materializing",
                    "queued",
                    "provisioning",
                    "running",
                    "cancelling",
                    "succeeded",
                    "failed",
                    "cancelled",
                    "expired",
                },
                kind,
            ),
        ),
        needs=_string_list(value, "needs", kind),
        source_revision=_nullable_string(value, "sourceRevision", kind),
        ready_at=_nullable_string(value, "readyAt", kind),
        queue_expires_at=_nullable_string(value, "queueExpiresAt", kind),
        terminal_at=_nullable_string(value, "terminalAt", kind),
        waiting_reason=_nullable_string(value, "waitingReason", kind),
        error_code=_nullable_string(value, "errorCode", kind),
    )


def _string_list(value: Mapping[str, object], name: str, kind: str) -> tuple[str, ...]:
    selected = value.get(name)
    if not isinstance(selected, list) or not all(isinstance(item, str) for item in selected):
        raise ProtocolError(f"Zeroshot emitted malformed {kind}.{name}")
    return tuple(selected)


def _nullable_string(value: Mapping[str, object], name: str, kind: str) -> str | None:
    if name not in value:
        raise ProtocolError(f"Zeroshot omitted {kind}.{name}")
    selected = value[name]
    if selected is not None and not isinstance(selected, str):
        raise ProtocolError(f"Zeroshot emitted malformed {kind}.{name}")
    return selected
