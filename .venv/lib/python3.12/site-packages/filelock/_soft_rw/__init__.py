"""Cross-process and cross-host reader/writer lock over a generation log of immutable snapshots."""

from __future__ import annotations

__lazy_modules__ = ["filelock._soft_rw._async"]

from filelock._soft_rw._async import AsyncAcquireSoftReadWriteReturnProxy, AsyncSoftReadWriteLock
from filelock._soft_rw._sync import SoftReadWriteLock

__all__ = [
    "AsyncAcquireSoftReadWriteReturnProxy",
    "AsyncSoftReadWriteLock",
    "SoftReadWriteLock",
]
