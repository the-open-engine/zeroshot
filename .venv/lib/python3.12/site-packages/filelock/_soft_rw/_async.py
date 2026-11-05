"""Async wrapper around :class:`SoftReadWriteLock` for use with ``asyncio``."""

from __future__ import annotations

__lazy_modules__ = ["asyncio", "filelock._async", "filelock._error", "functools", "threading"]

import asyncio
import functools
import os
import threading
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Final, Literal, ParamSpec, TypeVar

from filelock._async import (
    _TASKS_THREAD_ID,
    _BackendOutcome,
    _capture_call,
    _drain_future,
    _future_result,
    _raise_cancelled_error,
    _task_owners_for,
    _wait_until_done,
)
from filelock._error import Timeout
from filelock._soft_rw._sync import SoftReadWriteLock, _lost_hold_error

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Callable
    from concurrent import futures
    from types import TracebackType

    from filelock._api import AcquireReturnProxy
    from filelock._lease import LeaseCompromise

_P = ParamSpec("_P")
_R = TypeVar("_R")


class AsyncSoftReadWriteLock:
    """
    Async wrapper around :class:`SoftReadWriteLock` for ``asyncio`` applications.

    The sync class's blocking filesystem operations run on a thread pool via ``loop.run_in_executor()``. Each
    ``asyncio`` task owns its own hold. Tasks share a read lock, and a task asking for the write lock waits until the
    other holders release, with waiting writers ahead of new readers. Upgrading or downgrading a held lock raises
    :class:`RuntimeError`. The underlying :class:`SoftReadWriteLock` handles forks, the heartbeat, and stale eviction.
    Singleton wrappers share one sync lock and its task holds, and closing one ends only the holds taken through it.

    :param lock_file: path to the lock file; the protocol directory lives next to it as ``<lock_file>.rw``
    :param timeout: maximum wait time in seconds; ``-1`` means block indefinitely
    :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately on contention
    :param is_singleton: if ``True``, reuse existing :class:`SoftReadWriteLock` instances per resolved path
    :param heartbeat_interval: seconds between heartbeat refreshes; default 30 s
    :param stale_threshold: seconds a holder record may stay unchanged before a contender evicts it; defaults to
        ``3 * heartbeat_interval``
    :param poll_interval: seconds between acquire retries under contention; default 0.25 s
    :param on_compromise: called from the heartbeat thread with a :class:`~filelock.LeaseCompromise` when the hold is
        lost
    :param loop: event loop for ``run_in_executor``; ``None`` uses the running loop
    :param executor: executor for ``run_in_executor``; ``None`` uses the default executor

    .. versionadded:: 3.27.0

    """

    def __init__(  # ruff:ignore[too-many-arguments]  # public constructor: one parameter per documented lock option
        self,
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        *,
        blocking: bool = True,
        is_singleton: bool = True,
        heartbeat_interval: float = 30.0,
        stale_threshold: float | None = None,
        poll_interval: float = 0.25,
        on_compromise: Callable[[LeaseCompromise], None] | None = None,
        loop: asyncio.AbstractEventLoop | None = None,
        executor: futures.Executor | None = None,
    ) -> None:
        self._creator_pid = os.getpid()
        self._closed = False
        self._owns_backend = not is_singleton
        #: Marks the task holds taken through this wrapper, which its close() ends.
        self._via: Final = object()
        self._lock = SoftReadWriteLock(
            lock_file,
            timeout,
            blocking=blocking,
            is_singleton=is_singleton,
            heartbeat_interval=heartbeat_interval,
            stale_threshold=stale_threshold,
            poll_interval=poll_interval,
            on_compromise=on_compromise,
        )
        self._owners = _task_owners_for(self._lock)
        self._loop = loop
        self._executor = executor

    @property
    def lock_file(self) -> str:
        """The path to the lock file passed to the constructor."""
        return self._lock.lock_file

    @property
    def timeout(self) -> float:
        """The default timeout applied when ``acquire_read`` / ``acquire_write`` is called without one."""
        return self._lock.timeout

    @property
    def blocking(self) -> bool:
        """Whether ``acquire_*`` defaults to blocking; ``False`` makes contention raise immediately."""
        return self._lock.blocking

    @property
    def generation(self) -> int | None:
        """The generation the current hold was granted at, a fencing token; ``None`` when no lock is held."""
        return self._lock.generation

    @property
    def compromise(self) -> LeaseCompromise | None:
        """How the current hold was lost, or ``None`` while it stands; acquiring raises until the hold is released."""
        return self._lock.compromise

    @property
    def loop(self) -> asyncio.AbstractEventLoop | None:
        """The event loop used for ``run_in_executor``, or ``None`` for the running loop."""
        return self._loop

    @property
    def executor(self) -> futures.Executor | None:
        """The executor used for ``run_in_executor``, or ``None`` for the default executor."""
        return self._executor

    @asynccontextmanager
    async def read_lock(self, timeout: float | None = None, *, blocking: bool | None = None) -> AsyncGenerator[None]:
        """
        Async context manager that acquires and releases a shared read lock.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

        :raises RuntimeError: if the calling task already holds the write lock
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        await self.acquire_read(timeout, blocking=blocking)
        try:
            yield
        finally:
            await self.release()

    @asynccontextmanager
    async def write_lock(self, timeout: float | None = None, *, blocking: bool | None = None) -> AsyncGenerator[None]:
        """
        Async context manager that acquires and releases an exclusive write lock.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

        :raises RuntimeError: if the calling task already holds the read lock
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        await self.acquire_write(timeout, blocking=blocking)
        try:
            yield
        finally:
            await self.release()

    async def acquire_read(
        self, timeout: float | None = None, *, blocking: bool | None = None
    ) -> AsyncAcquireSoftReadWriteReturnProxy:
        """
        Acquire a shared read lock.

        See :meth:`SoftReadWriteLock.acquire_read` for reentrancy / upgrade / fork semantics. The blocking work runs
        inside ``run_in_executor`` so other coroutines on the same loop keep progressing while this call waits.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

        :returns: a proxy usable as an async context manager to release the lock

        :raises RuntimeError: if the calling task already holds the write lock, if this instance was invalidated by
            :func:`os.fork`, or if :meth:`close` was called
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        self._raise_if_unusable()
        await self._acquire("read", timeout, blocking=blocking)
        return AsyncAcquireSoftReadWriteReturnProxy(lock=self)

    async def acquire_write(
        self, timeout: float | None = None, *, blocking: bool | None = None
    ) -> AsyncAcquireSoftReadWriteReturnProxy:
        """
        Acquire an exclusive write lock.

        See :meth:`SoftReadWriteLock.acquire_write` for the writer-preferring semantics. The blocking work runs inside
        ``run_in_executor``.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

        :returns: a proxy usable as an async context manager to release the lock

        :raises RuntimeError: if the calling task already holds the read lock, if this instance was invalidated by
            :func:`os.fork`, or if :meth:`close` was called
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        self._raise_if_unusable()
        await self._acquire("write", timeout, blocking=blocking)
        return AsyncAcquireSoftReadWriteReturnProxy(lock=self)

    async def _acquire(self, mode: Literal["read", "write"], timeout: float | None, *, blocking: bool | None) -> None:
        blocking = self._lock.blocking if blocking is None else blocking

        async def enter(remaining: float) -> None:
            # A close() may have landed while this task waited for the table.
            self._raise_if_unusable()
            await self._run_acquire(
                functools.partial(
                    self._lock._acquire_level_of,  # ruff:ignore[private-member-access]  # the wrapper owns this backend hold
                    _TASKS_THREAD_ID,
                    mode,
                    remaining,
                    blocking=blocking,
                )
            )

        def joinable() -> bool:
            self._raise_if_unusable()
            if (compromise := self._lock.compromise) is not None:
                raise _lost_hold_error(self.lock_file, mode, compromise)
            return self._backend_holds()

        await self._owners.acquire(
            mode,
            timeout=self._lock.timeout if timeout is None else timeout,
            blocking=blocking,
            lock_file=self.lock_file,
            via=self._via,
            enter=enter,
            joinable=joinable,
        )

    async def release(self, *, force: bool = False) -> None:
        """
        Release one level of the calling task's hold.

        The wrapper releases the lock once no task holds it.

        :param force: if ``True``, end every level the calling task took through this wrapper. Levels taken through
            other wrappers on the same path stay. A wrapper that holds no level for the task releases nothing.

        :raises RuntimeError: if the calling task holds no level through this wrapper and *force* is ``False``

        """
        if self._creator_pid == os.getpid():
            await self._owners.release(
                force=force,
                lock_file=self.lock_file,
                via=self._via,
                leave=functools.partial(self._submit, self._leave_backend),
                still_held=self._backend_holds,
            )

    async def close(self) -> None:
        """
        End the holds of the tasks that acquired through this instance and release the lock once no task holds it.

        Idempotent. After calling this method the instance can no longer acquire locks. With ``is_singleton=False`` it
        also closes the underlying :class:`SoftReadWriteLock`. A singleton wrapper leaves the shared one open, so tasks
        that acquired through another wrapper and synchronous users of the same path keep their holds. The next acquire
        notices a synchronous ``close()`` or forced release of that shared lock. One that lands in the few bytecodes
        between that check and the admission still admits the task into the ended hold.

        """
        if self._creator_pid != os.getpid() or self._closed:
            return
        # Refuse acquires from here on, so none is admitted into a hold the backend is letting go of.
        self._closed = True
        try:
            await self._owners.retire(
                None if self._owns_backend else self._via,
                leave=functools.partial(self._submit, self._lock.close if self._owns_backend else self._leave_backend),
                still_held=self._backend_holds,
            )
        except BaseException as error:
            # Only a failed backend call can leave the level held, and then a retried close() has to find it.
            if not isinstance(error, asyncio.CancelledError) and self._backend_holds():
                self._closed = False
            raise

    def _raise_if_unusable(self) -> None:
        if self._creator_pid != os.getpid():  # pragma: forked child
            msg = f"AsyncSoftReadWriteLock on {self.lock_file} was inherited across fork; construct a new instance"
            raise RuntimeError(msg)
        if self._closed:
            msg = f"AsyncSoftReadWriteLock on {self.lock_file} has been closed"
            raise RuntimeError(msg)

    def _backend_holds(self) -> bool:
        # A sync close() or forced release on a shared singleton ends the tasks' level without the table taking part.
        return self._lock._holds_level_of(_TASKS_THREAD_ID)  # ruff:ignore[private-member-access]  # the wrapper owns this backend hold

    def _leave_backend(self) -> None:
        # Only the tasks' level: a sync user of the same singleton keeps its own.
        self._lock._release_level_of(_TASKS_THREAD_ID)  # ruff:ignore[private-member-access]  # the wrapper owns this backend hold

    async def _run_acquire(self, acquire: Callable[..., AcquireReturnProxy]) -> None:
        # run_in_executor cannot recall work the pool already started. A sync acquire that won the lock before it saw
        # the cancellation has published its claim and started the heartbeat, which keeps the record fresh so no peer
        # evicts it. Wait the call out and hand the claim back, as AsyncReadWriteLock does.
        abandoned = threading.Event()
        acquire_future = self._submit(acquire, cancel_check=abandoned.is_set)
        try:
            await _wait_until_done(acquire_future)
        except asyncio.CancelledError as cancellation:
            # Stops a backend wait on another process's hold, which would otherwise outlast the caller by as long.
            abandoned.set()
            try:
                await _drain_future(acquire_future)
            except Timeout:
                # Nothing was taken, so the caller's cancellation is the whole outcome.
                raise cancellation from None
            except BaseException as error:  # ruff:ignore[blind-except]  # reported with the cancellation below
                _raise_cancelled_error(cancellation, error)
            try:
                await _drain_future(self._submit(self._leave_backend))
            except BaseException as error:  # ruff:ignore[blind-except]  # reported with the cancellation below
                _raise_cancelled_error(cancellation, error)
            raise
        _future_result(acquire_future)

    def _submit(
        self, func: Callable[_P, _R], *args: _P.args, **kwargs: _P.kwargs
    ) -> asyncio.Future[_BackendOutcome[_R]]:
        loop = self._loop or asyncio.get_running_loop()
        return loop.run_in_executor(self._executor, _capture_call, functools.partial(func, *args, **kwargs))


class AsyncAcquireSoftReadWriteReturnProxy:
    """Async context-aware object that releases an :class:`AsyncSoftReadWriteLock` on exit."""

    def __init__(self, lock: AsyncSoftReadWriteLock) -> None:
        self.lock = lock

    async def __aenter__(self) -> AsyncSoftReadWriteLock:
        return self.lock

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.lock.release()


__all__ = [
    "AsyncAcquireSoftReadWriteReturnProxy",
    "AsyncSoftReadWriteLock",
]
