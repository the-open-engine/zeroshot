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
        map_indices: Zero-based map path, outermost first; empty outside maps.
    """

    execution: str
    node: str
    map_indices: tuple[int, ...] = ()


@dataclass(frozen=True, slots=True, kw_only=True)
class FailureDiagnostic:
    """One producer-redacted failure record retained with the run.

    Worker refusal messages distinguish authentication required, model unavailable, and stopped
    nodes. Messages pass through unchanged; this record also carries runtime failure diagnostics.

    Args:
        message: Bounded secret-safe explanation.
        execution: Associated executable graph node, or None for a run-wide record.
        cursor: Cursor of the original diagnostic log, when durably recorded.
    """

    message: str
    execution: ActiveExecution | None = None
    cursor: str | None = None


@dataclass(frozen=True, slots=True, kw_only=True)
class ExhaustedLoop:
    """A reducer-confirmed exhausted loop visit.

    Args:
        node: Graph loop name.
        completed_iterations: Completed rounds, independent of execution retries.
        map_indices: Zero-based map path, outermost first.
    """

    node: str
    completed_iterations: int
    map_indices: tuple[int, ...] = ()


@dataclass(frozen=True, slots=True, kw_only=True)
class FailureContext:
    """Recorded failure evidence without inferring causality from log order.

    Args:
        primary: Explicitly identified runtime failure, when available.
        recorded: Recent failure records, which may include recovered or voided attempts.
        verifiers: Latest recorded graph-defined results per verifier and map path.
        exhausted_loops: Reducer-confirmed exhausted loop visits.
        incomplete: Whether additional failure evidence may be missing from this summary.
    """

    primary: FailureDiagnostic | None = None
    recorded: tuple[FailureDiagnostic, ...] = ()
    verifiers: tuple[FailureDiagnostic, ...] = ()
    exhausted_loops: tuple[ExhaustedLoop, ...] = ()
    incomplete: bool = False


@dataclass(frozen=True, slots=True, kw_only=True)
class RunResult:
    """Terminal outcome from Client.run(), Run.wait(), or Run.force_stop().

    Args:
        run_id: Public durable run identity.
        succeeded: Whether the graph completed successfully.
        output: Successful graph-authored output, which may be JSON null.
        delivery_receipt: Native delivery evidence, independent of output; None when unavailable.
        failure: Stable nonempty failure reason when succeeded is false.
        context: Structured safe failure evidence, or None for older run data.
    """

    run_id: str
    succeeded: bool
    output: JsonValue = None
    delivery_receipt: JsonValue = None
    failure: str | None = None
    context: FailureContext | None = None

    def raise_for_failure(self) -> None:
        """Raise RunFailedError for a failed graph result; otherwise return None."""
        if not self.succeeded:
            from .run_errors import RunFailedError

            raise RunFailedError(self)


@dataclass(frozen=True, slots=True, kw_only=True)
class ControllerObservation:
    """Local controller availability, separate from durable run status.

    Args:
        state: lost when absence is confirmed, otherwise unknown.
        observed_at: Time of this check as Unix epoch milliseconds, not the death time.
    """

    state: Literal["lost", "unknown"]
    observed_at: int


@dataclass(frozen=True, slots=True, kw_only=True)
class RunStatus:
    """Current durable projection for one run.

    Args:
        run_id: Public durable run identity.
        title: Immutable persisted title.
        source: Immutable Git source snapshot, or None when unavailable for a local run.
        size: Opaque size identifier accepted by the target.
        cursor: Opaque durable status cursor.
        phase: admitted, running, stopping, suspended, or finished.
        active_executions: Every currently active graph execution.
        result: Terminal result only when phase is finished.
        graph_result: Recorded graph result while running or stopping, before finalization ends.
        controller: Availability observation, omitted for responding controllers and ended runs.
        suspend_requested: Whether a running run has a durable suspension request.
        reason: Why an ended attempt suspended.
        node: Node held back by suspension.
        created_at: Suspension time in Unix epoch milliseconds.
        due_at: Earliest continuation time for a suspended wait, in epoch milliseconds.
    """

    run_id: str
    title: str
    source: ResolvedSource | None
    size: str
    cursor: str
    phase: Literal["admitted", "running", "stopping", "suspended", "finished"]
    active_executions: tuple[ActiveExecution, ...] = ()
    result: RunResult | None = None
    graph_result: RunResult | None = None
    controller: ControllerObservation | None = None
    suspend_requested: bool = False
    reason: Literal["command", "wait"] | None = None
    node: str | None = None
    created_at: int | None = None
    due_at: int | None = None


@dataclass(frozen=True, slots=True, kw_only=True)
class RunSummary:
    """Inventory projection returned by Client.list_runs().

    Args:
        run_id: Public durable run identity.
        title: Immutable persisted title.
        source: Immutable Git source snapshot, or None when unavailable for a local run.
        size: Opaque size identifier accepted by the target.
        cursor: Opaque durable status cursor.
        phase: admitted, running, stopping, suspended, or finished.
        force_stop_requested: Whether the current projection reflects a force-stop request.
        graph_result: Recorded graph result while running or stopping, before finalization ends.
        controller: Availability observation, omitted for responding controllers and ended runs.
        suspend_requested: Whether a running run has a durable suspension request.
        reason: Why an ended attempt suspended.
        node: Node held back by suspension.
        created_at: Suspension time in Unix epoch milliseconds.
        due_at: Earliest continuation time for a suspended wait, in epoch milliseconds.
    """

    run_id: str
    title: str
    source: ResolvedSource | None
    size: str
    cursor: str
    phase: Literal["admitted", "running", "stopping", "suspended", "finished"]
    force_stop_requested: bool
    graph_result: RunResult | None = None
    controller: ControllerObservation | None = None
    suspend_requested: bool = False
    reason: Literal["command", "wait"] | None = None
    node: str | None = None
    created_at: int | None = None
    due_at: int | None = None


@dataclass(frozen=True, slots=True, kw_only=True)
class LogEvent:
    """One safe durable native log record.

    Args:
        run_id: Public durable run identity.
        cursor: Opaque durable log cursor.
        timestamp: Producer-captured Unix epoch milliseconds, preserved by replay.
        execution: Opaque execution selector, or None for run-wide records.
        level: Native debug, info, or error level.
        map_indices: Zero-based map path, outermost first; empty outside maps.
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
    map_indices: tuple[int, ...] = ()


@dataclass(frozen=True, slots=True, kw_only=True)
class RunVector:
    """One immutable entry in the run's ordered vector record.

    Args:
        id: Opaque ordered entry identity, preserved by continuation.
        run_id: Attempt that wrote the entry.
        created_at: Entry time in Unix epoch milliseconds.
        kind: Entry kind.
        content: Complete kind-specific JSON fields.
    """

    id: str
    run_id: str
    created_at: int
    kind: str
    content: JsonValue


@dataclass(frozen=True, slots=True, kw_only=True)
class RunVectorsPage:
    """A bounded page of the vector record.

    Args:
        run_id: Run whose record was read.
        vectors: Ordered entries with their complete content.
        next_after: Cursor for the next page, or None at the end.
    """

    run_id: str
    vectors: tuple[RunVector, ...]
    next_after: str | None
