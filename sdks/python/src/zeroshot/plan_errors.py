"""Merge-plan-specific public exceptions for the Zeroshot Python SDK."""

from __future__ import annotations

from typing import Protocol

from .errors import ZeroshotError


class _MergePlanHandle(Protocol):
    @property
    def id(self) -> str: ...


class MergePlanWaitTimeout(ZeroshotError):
    """Raised when observation times out while a durable merge plan remains active.

    Args:
        plan: Durable plan handle that can resume observation while its client remains open.
        wait_timeout: Caller-supplied non-negative deadline in seconds.
    """

    def __init__(self, plan: _MergePlanHandle, wait_timeout: float) -> None:
        super().__init__(f"merge plan {plan.id} did not finish within {wait_timeout} seconds")
        self.plan = plan
        self.wait_timeout = wait_timeout
