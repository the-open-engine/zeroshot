"""Strict projections from native JSON into public immutable read models."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Literal, TypeAlias, TypedDict, cast

from .errors import ProtocolError
from .runs import (
    ActiveExecution,
    ControllerObservation,
    ExhaustedLoop,
    FailureContext,
    FailureDiagnostic,
    LogEvent,
    RunResult,
    RunStatus,
    RunSummary,
    RunVector,
    RunVectorsPage,
)
from .source import ResolvedGitRemote, ResolvedSource, SourceForgeIdentity
from .stacks import StackRunStatus, StackStatus
from .values import JsonValue

_Phase: TypeAlias = Literal["admitted", "running", "stopping", "suspended", "finished"]
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
            {"admitted", "running", "stopping", "suspended", "finished"},
            "run status.status",
        ),
    )
    active = tuple(
        ActiveExecution(
            execution=_string(item, "execution", "active execution"),
            node=_string(item, "node", "active execution"),
            map_indices=_map_indices(item),
        )
        for item in _mapping_list(
            native_status.get("activeExecutions", []),
            "run status.status.activeExecutions",
        )
    )
    result = None
    graph_result = None
    if phase in {"running", "stopping"} and native_status.get("graphResult") is not None:
        graph_result = _terminal_result(
            _string(root, "runId", "run status"),
            native_status["graphResult"],
            None,
        )
    if phase == "finished":
        result = _terminal_result(
            _string(root, "runId", "run status"),
            native_status.get("terminalResult"),
            native_status.get("metadata"),
        )
    return RunStatus(
        run_id=_string(root, "runId", "run status"),
        title=_string(root, "title", "run status"),
        source=_source(root.get("source")),
        size=_string(root, "size", "run status"),
        cursor=_cursor(root),
        phase=phase,
        active_executions=active,
        result=result,
        graph_result=graph_result,
        controller=_controller(root.get("controller")),
        **_suspension(native_status, phase),
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
        graph_result=status.graph_result,
        controller=status.controller,
        suspend_requested=status.suspend_requested,
        reason=status.reason,
        node=status.node,
        created_at=status.created_at,
        due_at=status.due_at,
    )


def _controller(value: object) -> ControllerObservation | None:
    if value is None:
        return None
    root = _mapping(value, "run status.controller")
    return ControllerObservation(
        state=cast(
            Literal["lost", "unknown"],
            _enum_string(root, "state", {"lost", "unknown"}, "run status.controller"),
        ),
        observed_at=_positive_javascript_safe_integer(root, "observedAt", "run status.controller"),
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
        map_indices=_map_indices(root),
    )


def _source(value: object) -> ResolvedSource | None:
    if value is None:
        return None
    source = _mapping(value, "run status.source")
    return ResolvedSource(
        repository=_string(source, "repository", "run status.source"),
        branch=_string(source, "branch", "run status.source"),
        revision=_string(source, "revision", "run status.source"),
        remote=_remote(source.get("remote")),
    )


def _remote(value: object) -> ResolvedGitRemote | None:
    if value is None:
        return None
    remote = _mapping(value, "run status.source.remote")
    forge_value = remote.get("forge")
    forge = None
    if forge_value is not None:
        identity = _mapping(forge_value, "run status.source.remote.forge")
        forge = SourceForgeIdentity(
            provider=_string(identity, "provider", "run status.source.remote.forge"),
            repository=_string(identity, "repository", "run status.source.remote.forge"),
        )
    connection = remote.get("connection")
    if connection is not None and not isinstance(connection, str):
        raise ProtocolError("Zeroshot emitted malformed run status.source.remote.connection")
    return ResolvedGitRemote(
        url=_string(remote, "url", "run status.source.remote"),
        forge=forge,
        connection=connection,
    )


def _terminal_result(run_id: str, value: object, metadata: object = None) -> RunResult:
    receipt = None
    if metadata is not None:
        fields = _mapping(metadata, "run status.status.metadata")
        if fields.get("deliveryReceipt") is not None:
            receipt = cast(JsonValue, dict(_mapping(fields["deliveryReceipt"], "delivery receipt")))
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
            delivery_receipt=receipt,
            output=cast(JsonValue, terminal["output"]),
        )
    return RunResult(
        run_id=run_id,
        succeeded=False,
        delivery_receipt=receipt,
        failure=_string(terminal, "reason", "run status.status.terminalResult"),
        context=_failure_context(terminal.get("context")),
    )


def _map_indices(value: Mapping[str, object]) -> tuple[int, ...]:
    indices = value.get("mapIndices", [])
    if not isinstance(indices, list) or any(
        type(index) is not int or not 0 <= index <= 2**53 - 1 for index in indices
    ):
        raise ProtocolError("Zeroshot emitted malformed mapIndices")
    return tuple(indices)


def _completed_iterations(value: Mapping[str, object]) -> int:
    count = value.get("completedIterations")
    if type(count) is not int or not 1 <= count <= 2**53 - 1:
        raise ProtocolError("Zeroshot emitted malformed completedIterations")
    return count


def _failure_context(value: object) -> FailureContext | None:
    if value is None:
        return None
    context = _mapping(value, "failure context")
    primary = context.get("primary")
    incomplete = context.get("incomplete", False)
    if not isinstance(incomplete, bool):
        raise ProtocolError("Zeroshot emitted malformed failure context.incomplete")
    return FailureContext(
        primary=_failure_diagnostic(primary) if primary is not None else None,
        recorded=tuple(
            _failure_diagnostic(item)
            for item in _mapping_list(context.get("recorded", []), "failure context.recorded")
        ),
        verifiers=tuple(
            _failure_diagnostic(item)
            for item in _mapping_list(context.get("verifiers", []), "failure context.verifiers")
        ),
        exhausted_loops=tuple(
            ExhaustedLoop(
                node=_string(item, "node", "exhausted loop"),
                completed_iterations=_completed_iterations(item),
                map_indices=_map_indices(item),
            )
            for item in _mapping_list(
                context.get("exhaustedLoops", []), "failure context.exhaustedLoops"
            )
        ),
        incomplete=incomplete,
    )


def _failure_diagnostic(value: object) -> FailureDiagnostic:
    diagnostic = _mapping(value, "failure diagnostic")
    execution = diagnostic.get("execution")
    active = None
    if execution is not None:
        identity = _mapping(execution, "failure diagnostic.execution")
        active = ActiveExecution(
            execution=_string(identity, "execution", "failure diagnostic.execution"),
            node=_string(identity, "node", "failure diagnostic.execution"),
            map_indices=_map_indices(identity),
        )
    cursor = diagnostic.get("cursor")
    if cursor is not None and not isinstance(cursor, str):
        raise ProtocolError("Zeroshot emitted malformed failure diagnostic.cursor")
    return FailureDiagnostic(
        message=_string(diagnostic, "message", "failure diagnostic"),
        execution=active,
        cursor=cursor,
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


def _stack_status(value: object) -> StackStatus:
    kind = "stack status"
    root = _mapping(value, kind)
    runs = tuple(
        _stack_run_status(item) for item in _mapping_list(root.get("runs"), f"{kind}.runs")
    )
    return StackStatus(
        stack_id=_string(root, "stackId", kind),
        title=_string(root, "title", kind),
        state=cast(
            Literal["queued", "running", "succeeded", "failed", "cancelled", "expired"],
            _enum_string(
                root,
                "state",
                {"queued", "running", "succeeded", "failed", "cancelled", "expired"},
                kind,
            ),
        ),
        repository=_string(root, "repository", kind),
        branch=_string(root, "branch", kind),
        submitted_at=_string(root, "submittedAt", kind),
        expires_at=_string(root, "expiresAt", kind),
        runs=runs,
    )


def _stack_run_status(value: Mapping[str, object]) -> StackRunStatus:
    kind = "stack status.runs"
    return StackRunStatus(
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
        source_branch=_nullable_string(value, "sourceBranch", kind),
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


class _SuspensionFields(TypedDict, total=False):
    suspend_requested: bool
    reason: Literal["command", "wait"]
    node: str
    created_at: int
    due_at: int | None


def _suspension(value: Mapping[str, object], phase: str) -> _SuspensionFields:
    requested = value.get("suspendRequested", False)
    if not isinstance(requested, bool):
        raise ProtocolError("Zeroshot emitted malformed suspendRequested")
    if phase != "suspended":
        return {"suspend_requested": requested}
    wait = value.get("wait")
    reason = cast(
        Literal["command", "wait"],
        _enum_string(value, "reason", {"command", "wait"}, "suspension"),
    )
    if (reason == "wait") != (wait is not None):
        raise ProtocolError("Zeroshot emitted malformed suspension.wait")
    return {
        "suspend_requested": requested,
        "reason": reason,
        "node": _string(value, "node", "suspension"),
        "created_at": _positive_javascript_safe_integer(value, "createdAt", "suspension"),
        "due_at": None
        if wait is None
        else _positive_javascript_safe_integer(
            _mapping(wait, "suspension.wait"), "dueAt", "suspension.wait"
        ),
    }


def _vector(value: object) -> RunVector:
    entry = _mapping(value, "vector")
    return RunVector(
        id=_string(entry, "id", "vector"),
        run_id=_string(entry, "runId", "vector"),
        created_at=_positive_javascript_safe_integer(entry, "createdAt", "vector"),
        kind=_string(entry, "kind", "vector"),
        content=cast(JsonValue, dict(entry)),
    )


def _vectors(value: object) -> RunVectorsPage:
    page = _mapping(value, "vector page")
    return RunVectorsPage(
        run_id=_string(page, "runId", "vector page"),
        vectors=tuple(_vector(item) for item in _mapping_list(page.get("vectors"), "vectors")),
        next_after=_nullable_string(
            {"nextAfter": page.get("nextAfter")}, "nextAfter", "vector page"
        ),
    )
