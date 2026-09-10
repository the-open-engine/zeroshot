"""Immutable hosted merge-plan request and status models."""

from __future__ import annotations

from collections.abc import Mapping
from copy import deepcopy
from dataclasses import dataclass
from types import MappingProxyType
from typing import Literal

from .values import JsonValue

_PLAN_SCHEMA = "zeroshot.merge-plan/v1"
_PLAN_TERMINAL_STATES = frozenset({"succeeded", "failed", "cancelled", "expired"})


@dataclass(frozen=True, slots=True, kw_only=True)
class MergePlanRun:
    """One statically defined node in a hosted merge plan.

    Args:
        input: Closed JSON input validated against the selected profile graph.
        needs: Symbolic node names that must merge successfully before this node becomes ready.
    """

    input: JsonValue
    needs: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "input", deepcopy(self.input))

    def to_dict(self) -> dict[str, JsonValue]:
        """Return this node's exact manifest representation."""
        value: dict[str, JsonValue] = {"input": deepcopy(self.input)}
        if self.needs:
            value["needs"] = list(self.needs)
        return value


@dataclass(frozen=True, slots=True, kw_only=True)
class MergePlanRequest:
    """A complete merge-only DAG submitted atomically to one hosted target.

    Args:
        title: Human-readable immutable plan title.
        repository: Source repository in owner/name form, shared by every node.
        branch: Source branch shared by every node.
        profile: Hosted profile selector in user:name or org:name form.
        expires_at: RFC 3339 deadline for the whole plan, at most seven days in the future.
        runs: Node names mapped to static inputs and symbolic dependencies.
        submission_key: Stable idempotency key; None generates one before native validation.
    """

    title: str
    repository: str
    branch: str
    profile: str
    expires_at: str
    runs: Mapping[str, MergePlanRun]
    submission_key: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "runs", MappingProxyType(dict(self.runs)))

    def to_dict(self) -> dict[str, JsonValue]:
        """Return the strict public merge-plan manifest consumed by Zeroshot."""
        nodes: dict[str, JsonValue] = {name: run.to_dict() for name, run in self.runs.items()}
        return {
            "schema": _PLAN_SCHEMA,
            "title": self.title,
            "source": {"repository": self.repository, "branch": self.branch},
            "profile": self.profile,
            "expiresAt": self.expires_at,
            "runs": nodes,
        }


@dataclass(frozen=True, slots=True, kw_only=True)
class MergePlanRunStatus:
    """Current lifecycle projection for one hosted merge-plan node.

    Args:
        name: Stable symbolic node name from the manifest.
        run_id: Server-assigned public run identity.
        state: Current materialization or run state.
        needs: Stable symbolic dependency names.
        source_revision: Exact source revision once the node materializes.
        ready_at: RFC 3339 time at which all dependencies had succeeded.
        queue_expires_at: RFC 3339 per-node queue deadline, set 24 hours after readiness.
        terminal_at: RFC 3339 terminal time.
        waiting_reason: Stable explanation while queued or blocked, when supplied.
        error_code: Stable terminal error category, including dependency_failed.
    """

    name: str
    run_id: str
    state: Literal[
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
    ]
    needs: tuple[str, ...]
    source_revision: str | None
    ready_at: str | None
    queue_expires_at: str | None
    terminal_at: str | None
    waiting_reason: str | None
    error_code: str | None

    @property
    def terminal(self) -> bool:
        """Whether this node can no longer change state."""
        return self.state in _PLAN_TERMINAL_STATES


@dataclass(frozen=True, slots=True, kw_only=True)
class MergePlanStatus:
    """Current aggregate projection for an immutable hosted merge plan.

    Args:
        plan_id: Server-assigned immutable plan identity.
        title: Human-readable immutable plan title.
        state: Aggregate plan state.
        repository: Source repository from the submitted manifest, shared by every node.
        branch: Source branch shared by every node.
        submitted_at: RFC 3339 admission time.
        expires_at: RFC 3339 whole-plan deadline.
        runs: Stable server-assigned node receipts and their current states.
    """

    plan_id: str
    title: str
    state: Literal["queued", "running", "succeeded", "failed", "cancelled", "expired"]
    repository: str
    branch: str
    submitted_at: str
    expires_at: str
    runs: tuple[MergePlanRunStatus, ...]

    @property
    def terminal(self) -> bool:
        """Whether every possible transition for this plan has completed."""
        return self.state in _PLAN_TERMINAL_STATES

    @property
    def succeeded(self) -> bool:
        """Whether the complete plan merged successfully."""
        return self.state == "succeeded"
