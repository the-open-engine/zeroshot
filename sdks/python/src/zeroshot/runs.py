"""Immutable run request, observation, and result models."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from .runtime import GraphSpec, RuntimePlan
from .source import ResolvedSource
from .values import JsonValue


@dataclass(frozen=True, slots=True, kw_only=True)
class RunRequest:
    """Inputs for one custom Zeroshot graph run.

    Args:
        title: Human-readable persisted run title.
        graph: Opaque GraphSpec passed to Zeroshot without changes.
        initial_input: Closed JSON input that Zeroshot validates against the graph.
        runtime: Opaque RuntimePlan passed to Zeroshot without changes.
        cwd: Working folder relative to the captured client workspace; None uses that workspace.
        repository: Source override as a Git clone URL or GitHub owner/name shorthand.
        source_connection: Explicit source-only named connection key.
        branch: Named-target source branch override.
        revision: Named-target exact source commit override.
        submission_key: Stable idempotency key; None generates one before native preflight.
    """

    title: str
    graph: GraphSpec
    initial_input: JsonValue
    runtime: RuntimePlan
    cwd: str | Path | None = None
    repository: str | None = None
    source_connection: str | None = None
    branch: str | None = None
    revision: str | None = None
    submission_key: str | None = None


@dataclass(frozen=True, slots=True, kw_only=True)
class ActiveExecution:
    """One graph-visible active execution.

    Args:
        execution: Opaque selector accepted by log filtering.
        node: Executable graph node name.
    """

    execution: str
    node: str


@dataclass(frozen=True, slots=True, kw_only=True)
class FailureDiagnostic:
    """One producer-redacted failure record retained with the run.

    Args:
        message: Bounded secret-safe explanation.
        execution: Associated executable graph node, or None for a run-wide record.
        cursor: Cursor of the original diagnostic log, when durably recorded.
    """

    message: str
    execution: ActiveExecution | None = None
    cursor: str | None = None


@dataclass(frozen=True, slots=True, kw_only=True)
class FailureContext:
    """Recorded failure evidence without inferring causality from log order.

    Args:
        primary: Explicitly identified runtime failure, when available.
        recorded: Recent failure records, which may include recovered or voided attempts.
        incomplete: Whether additional failure evidence may be missing from this summary.
    """

    primary: FailureDiagnostic | None = None
    recorded: tuple[FailureDiagnostic, ...] = ()
    incomplete: bool = False


@dataclass(frozen=True, slots=True, kw_only=True)
class RunResult:
    """Terminal outcome from Client.run(), Run.wait(), or Run.force_stop().

    Args:
        run_id: Public durable run identity.
        succeeded: Whether the graph completed successfully.
        output: Successful graph output, which may be JSON null.
        failure: Stable nonempty failure reason when succeeded is false.
        context: Structured safe failure evidence, or None for older run data.
    """

    run_id: str
    succeeded: bool
    output: JsonValue = None
    failure: str | None = None
    context: FailureContext | None = None

    def raise_for_failure(self) -> None:
        """Raise RunFailedError for a failed graph result; otherwise return None."""
        if not self.succeeded:
            from .run_errors import RunFailedError

            raise RunFailedError(self)


@dataclass(frozen=True, slots=True, kw_only=True)
class RunStatus:
    """Current durable projection for one run.

    Args:
        run_id: Public durable run identity.
        title: Immutable persisted title.
        source: Immutable Git source snapshot, or None when unavailable for a local run.
        size: Native run size.
        cursor: Opaque durable status cursor.
        phase: admitted, running, stopping, or finished.
        active_executions: Every currently active graph execution.
        result: Terminal result only when phase is finished.
    """

    run_id: str
    title: str
    source: ResolvedSource | None
    size: Literal["small", "medium", "large"]
    cursor: str
    phase: Literal["admitted", "running", "stopping", "finished"]
    active_executions: tuple[ActiveExecution, ...] = ()
    result: RunResult | None = None


@dataclass(frozen=True, slots=True, kw_only=True)
class RunSummary:
    """Inventory projection returned by Client.list_runs().

    Args:
        run_id: Public durable run identity.
        title: Immutable persisted title.
        source: Immutable Git source snapshot, or None when unavailable for a local run.
        size: Native run size.
        cursor: Opaque durable status cursor.
        phase: admitted, running, stopping, or finished.
        force_stop_requested: Whether the current projection reflects a force-stop request.
    """

    run_id: str
    title: str
    source: ResolvedSource | None
    size: Literal["small", "medium", "large"]
    cursor: str
    phase: Literal["admitted", "running", "stopping", "finished"]
    force_stop_requested: bool


@dataclass(frozen=True, slots=True, kw_only=True)
class LogEvent:
    """One safe durable native log record.

    Args:
        run_id: Public durable run identity.
        cursor: Opaque durable log cursor.
        timestamp: Producer-captured Unix epoch milliseconds, preserved by replay.
        execution: Opaque execution selector, or None for run-wide records.
        level: Native debug, info, or error level.
        target: Bounded native log target.
        message: Bounded secret-safe message.
    """

    run_id: str
    cursor: str
    timestamp: int
    execution: str | None
    level: Literal["debug", "info", "error"]
    target: str
    message: str
