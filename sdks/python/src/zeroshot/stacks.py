"""Immutable hosted stack request and status models."""

from __future__ import annotations

from collections.abc import Mapping
from copy import deepcopy
from dataclasses import dataclass
from types import MappingProxyType
from typing import Literal, TypeAlias

from .values import JsonValue

_STACK_SCHEMA = "zeroshot.stack/v1"
_STACK_TERMINAL_STATES = frozenset({"succeeded", "failed", "cancelled", "expired"})

StackDelivery: TypeAlias = Literal["merge", "stacked_pull_requests"]
"""How a hosted stack lands its runs.

"merge" merges each run into the base branch before its dependents start. "stacked_pull_requests"
orders the runs into one linear stack and opens each pull request against the previous run's
branch without merging anything.
"""


@dataclass(frozen=True, slots=True, kw_only=True)
class StackRun:
    """One statically defined run in a hosted stack.

    Args:
        input: Closed JSON input validated against the selected profile graph.
        needs: Symbolic run names that must succeed before this run becomes ready. With "merge"
            delivery each of them has merged into the base branch first. With
            "stacked_pull_requests" delivery they only order the linear stack, in which each run
            needs exactly the run before it.
    """

    input: JsonValue
    needs: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "input", deepcopy(self.input))

    def to_dict(self) -> dict[str, JsonValue]:
        """Return this run's exact manifest representation."""
        value: dict[str, JsonValue] = {"input": deepcopy(self.input)}
        if self.needs:
            value["needs"] = list(self.needs)
        return value


@dataclass(frozen=True, slots=True, kw_only=True)
class StackRequest:
    """An immutable DAG of runs submitted atomically to one hosted target.

    Args:
        title: Human-readable immutable stack title.
        repository: Source repository in owner/name form, shared by every run.
        branch: Base branch shared by every run.
        profile: Hosted profile selector in user:name or org:name form. Agent bindings cannot
            declare GH_TOKEN; only the Git delivery binding can declare it.
        delivery: How the stack lands. "merge" requires exactly one
            builtin.git-delivery.merge@2 or builtin.git-delivery.merge@3 node in the profile and
            merges each run into the base branch before its dependents start; a run may need
            several others. "stacked_pull_requests" requires exactly one
            builtin.git-delivery.pr@2 node, turns the runs into one linear stack in dependency
            order with ties broken by run name, and opens each run's pull request against the
            previous run's branch, the first against the base branch. Nothing is merged.
        expires_at: RFC 3339 deadline for the whole stack, at most seven days in the future.
        runs: Run names mapped to static inputs and symbolic dependencies.
        submission_key: Stable idempotency key; None generates one before native validation.
    """

    title: str
    repository: str
    branch: str
    profile: str
    delivery: StackDelivery
    expires_at: str
    runs: Mapping[str, StackRun]
    submission_key: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "runs", MappingProxyType(dict(self.runs)))

    def to_dict(self) -> dict[str, JsonValue]:
        """Return the strict public stack manifest consumed by Zeroshot."""
        runs: dict[str, JsonValue] = {name: run.to_dict() for name, run in self.runs.items()}
        return {
            "schema": _STACK_SCHEMA,
            "title": self.title,
            "source": {"repository": self.repository, "branch": self.branch},
            "profile": self.profile,
            "delivery": self.delivery,
            "expiresAt": self.expires_at,
            "runs": runs,
        }


@dataclass(frozen=True, slots=True, kw_only=True)
class StackRunStatus:
    """Current lifecycle projection for one run in a hosted stack.

    Args:
        name: Stable symbolic run name from the manifest.
        run_id: Server-assigned public run identity.
        state: Current materialization or run state.
        needs: Stable symbolic dependency names.
        source_branch: Source branch the run started from once it materializes; in stacked
            pull-request delivery this is the previous run's pull-request branch, which its pull
            request targets.
        source_revision: Exact source revision once the run materializes.
        ready_at: RFC 3339 time when source materialization completed and the run became ready.
        queue_expires_at: RFC 3339 per-run queue deadline, set to the earlier of the stack deadline
            and seven days after readiness.
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
    source_branch: str | None
    source_revision: str | None
    ready_at: str | None
    queue_expires_at: str | None
    terminal_at: str | None
    waiting_reason: str | None
    error_code: str | None

    @property
    def terminal(self) -> bool:
        """Whether this run can no longer change state."""
        return self.state in _STACK_TERMINAL_STATES


@dataclass(frozen=True, slots=True, kw_only=True)
class StackStatus:
    """Current aggregate projection for an immutable hosted stack.

    Args:
        stack_id: Server-assigned immutable stack identity.
        title: Human-readable immutable stack title.
        state: Aggregate stack state.
        repository: Source repository from the submitted manifest, shared by every run.
        branch: Base branch shared by every run.
        submitted_at: RFC 3339 admission time.
        expires_at: RFC 3339 whole-stack deadline.
        runs: Stable server-assigned run receipts and their current states.
    """

    stack_id: str
    title: str
    state: Literal["queued", "running", "succeeded", "failed", "cancelled", "expired"]
    repository: str
    branch: str
    submitted_at: str
    expires_at: str
    runs: tuple[StackRunStatus, ...]

    @property
    def terminal(self) -> bool:
        """Whether every possible transition for this stack has completed."""
        return self.state in _STACK_TERMINAL_STATES

    @property
    def succeeded(self) -> bool:
        """Whether every run in the stack was delivered successfully."""
        return self.state == "succeeded"
