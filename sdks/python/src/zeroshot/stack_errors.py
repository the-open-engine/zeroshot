"""Stack-specific public exceptions for the Zeroshot Python SDK."""

from __future__ import annotations

from typing import Protocol

from .errors import ZeroshotError


class _StackHandle(Protocol):
    @property
    def id(self) -> str: ...


class StackWaitTimeout(ZeroshotError):
    """Raised when observation times out while a durable hosted stack remains active.

    Args:
        stack: Durable stack handle that can resume observation while its client remains open.
        wait_timeout: Caller-supplied non-negative deadline in seconds.
    """

    def __init__(self, stack: _StackHandle, wait_timeout: float) -> None:
        super().__init__(f"stack {stack.id} did not finish within {wait_timeout} seconds")
        self.stack = stack
        self.wait_timeout = wait_timeout
