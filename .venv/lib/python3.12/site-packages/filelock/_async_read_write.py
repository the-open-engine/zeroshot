"""Async wrapper around :class:`ReadWriteLock` for use with ``asyncio``."""

from __future__ import annotations

__lazy_modules__ = ["asyncio", "concurrent.futures", "filelock._async", "filelock._error", "functools", "threading"]

import asyncio
import functools
import os
import sqlite3
import threading
from concurrent.futures import ThreadPoolExecutor
from concurrent.futures import thread as executor_threads
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Final, Literal, ParamSpec, TypeVar, cast

from filelock._api import (
    _append_exception_context,
    _ensure_current_process,
    _fork_transition,
    _register_fork_object,
)
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
from filelock._read_write import ReadWriteLock

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Callable, MutableMapping
    from concurrent import futures
    from types import TracebackType

    from filelock._api import AcquireReturnProxy


_P = ParamSpec("_P")
_R = TypeVar("_R")


class AsyncReadWriteLock:
    """
    Async wrapper around :class:`ReadWriteLock` for use in ``asyncio`` applications.

    This wrapper dispatches every blocking SQLite operation to a thread pool via ``loop.run_in_executor()`` because
    Python's :mod:`sqlite3` module has no async API. Each ``asyncio`` task owns its own hold. Tasks share a read lock,
    and a task asking for the write lock waits until the other holders release, with waiting writers ahead of new
    readers. As with :class:`ReadWriteLock`, upgrading or downgrading a held lock raises :class:`RuntimeError`.
    Singleton wrappers over one path share their task holds, and closing one ends only the holds taken through it.

    :param lock_file: path to the SQLite database file used as the lock
    :param timeout: maximum wait time in seconds; ``-1`` waits up to SQLite's busy-timeout cap of about 23 days
    :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately when the lock is unavailable
    :param is_singleton: if ``True``, reuse existing :class:`ReadWriteLock` instances for the same resolved path
    :param loop: event loop for ``run_in_executor``; ``None`` uses the running loop
    :param executor: executor for ``run_in_executor``. When ``None`` this lock creates and owns a dedicated
        single-thread executor so every operation runs on the same thread (SQLite affinity requires this) and shuts it
        down in :meth:`close`. This lock uses a caller-supplied executor as-is and never shuts it down, so after passing
        no executor call :meth:`close` to release the owned one.

    .. versionadded:: 3.21.0

    """

    def __init__(  # ruff:ignore[too-many-arguments]  # public constructor: one parameter per documented lock option
        self,
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        *,
        blocking: bool = True,
        is_singleton: bool = True,
        loop: asyncio.AbstractEventLoop | None = None,
        executor: futures.Executor | None = None,
    ) -> None:
        creator_pid = os.getpid()
        self._creator_pid = creator_pid
        self._fork_invalidated = False
        self._closed = False
        self._owns_backend = not is_singleton
        #: Marks the task holds taken through this wrapper, which its close() ends.
        self._via: Final = object()
        _register_fork_object(self)
        with _fork_transition():
            self._lock = ReadWriteLock(lock_file, timeout, blocking=blocking, is_singleton=is_singleton)
            self._owners = _task_owners_for(self._lock)
            self._loop = loop
            self._owns_executor = executor is None
            self._executor = executor or ThreadPoolExecutor(max_workers=1)
            if os.getpid() != creator_pid:  # pragma: forked child
                msg = "AsyncReadWriteLock construction cannot continue after fork"
                raise RuntimeError(msg)

    @property
    def lock_file(self) -> str:
        """The path to the lock file."""
        return self._lock.lock_file

    @property
    def timeout(self) -> float:
        """The default timeout."""
        return self._lock.timeout

    @property
    def blocking(self) -> bool:
        """Whether blocking is enabled by default."""
        return self._lock.blocking

    @property
    def loop(self) -> asyncio.AbstractEventLoop | None:
        """The event loop (or ``None`` for the running loop)."""
        return self._loop

    @property
    def executor(self) -> futures.Executor:
        """The executor used for ``run_in_executor`` (a dedicated single-thread one if none was supplied)."""
        return self._executor

    @asynccontextmanager
    async def read_lock(self, timeout: float | None = None, *, blocking: bool | None = None) -> AsyncGenerator[None]:
        """
        Async context manager that acquires and releases a shared read lock.

        Falls back to instance defaults for *timeout* and *blocking* when ``None``.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

        """
        await self.acquire_read(timeout, blocking=blocking)
        body_error: BaseException | None = None
        try:
            yield
        except BaseException as error:
            body_error = error
            raise
        finally:
            await self._release_in_context(body_error)

    @asynccontextmanager
    async def write_lock(self, timeout: float | None = None, *, blocking: bool | None = None) -> AsyncGenerator[None]:
        """
        Async context manager that acquires and releases an exclusive write lock.

        Falls back to instance defaults for *timeout* and *blocking* when ``None``.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

        """
        await self.acquire_write(timeout, blocking=blocking)
        body_error: BaseException | None = None
        try:
            yield
        except BaseException as error:
            body_error = error
            raise
        finally:
            await self._release_in_context(body_error)

    async def _release_in_context(self, body_error: BaseException | None) -> None:
        try:
            await self.release()
        except BaseException as release_error:
            if body_error is not None:
                _append_exception_context(release_error, body_error)
            raise

    async def acquire_read(
        self, timeout: float | None = None, *, blocking: bool | None = None
    ) -> AsyncAcquireReadWriteReturnProxy:
        """
        Acquire a shared read lock.

        See :meth:`ReadWriteLock.acquire_read` for full semantics.

        :param timeout: seconds to wait; ``None`` uses the instance setting; ``-1`` waits up to SQLite's
            busy-timeout cap of about 23 days
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` on contention;
            ``None`` uses the instance setting

        :returns: a proxy that can be used as an async context manager to release the lock

        :raises RuntimeError: if the calling task already holds the write lock
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        self._raise_if_unusable()
        await self._acquire("read", timeout, blocking=blocking)
        return AsyncAcquireReadWriteReturnProxy(lock=self)

    async def acquire_write(
        self, timeout: float | None = None, *, blocking: bool | None = None
    ) -> AsyncAcquireReadWriteReturnProxy:
        """
        Acquire an exclusive write lock.

        See :meth:`ReadWriteLock.acquire_write` for full semantics.

        :param timeout: seconds to wait; ``None`` uses the instance setting; ``-1`` waits up to SQLite's
            busy-timeout cap of about 23 days
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` on contention;
            ``None`` uses the instance setting

        :returns: a proxy that can be used as an async context manager to release the lock

        :raises RuntimeError: if the calling task already holds the read lock
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        self._raise_if_unusable()
        await self._acquire("write", timeout, blocking=blocking)
        return AsyncAcquireReadWriteReturnProxy(lock=self)

    async def _acquire(self, mode: Literal["read", "write"], timeout: float | None, *, blocking: bool | None) -> None:
        timeout = self.timeout if timeout is None else timeout
        blocking = self.blocking if blocking is None else blocking

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
            return self._backend_holds()

        await self._owners.acquire(
            mode,
            timeout=timeout,
            blocking=blocking,
            lock_file=self.lock_file,
            via=self._via,
            enter=enter,
            joinable=joinable,
        )

    async def release(self, *, force: bool = False) -> None:
        """
        Release one level of the calling task's hold.

        The wrapper releases the database lock once no task holds it.

        :param force: if ``True``, end every level the calling task took through this wrapper. Levels taken through
            other wrappers on the same path stay. A wrapper that holds no level for the task releases nothing.

        :raises RuntimeError: if the calling task holds no level through this wrapper and *force* is ``False``

        """
        _ensure_current_process()
        if self._inherited:  # pragma: needs fork
            return
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

        After calling this method, the lock instance is no longer usable. With ``is_singleton=False`` it also closes the
        underlying SQLite connection. A singleton wrapper leaves the shared :class:`ReadWriteLock` open, so tasks that
        acquired through another wrapper and synchronous users of the same path keep their holds. The next acquire
        notices a synchronous ``close()`` or forced release of that shared lock. One that lands in the few bytecodes
        between that check and the admission still admits the task into the ended hold.

        """
        _ensure_current_process()
        if self._inherited:  # pragma: needs fork
            return
        if self._closed:
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
            else:
                self._shutdown_owned_executor()
            raise
        # Wait for the worker to exit rather than letting it drain in the background: a caller that forks right
        # after closing deserves a single-threaded process, and os.fork warns about any surviving thread.
        if self._owns_executor:
            await asyncio.to_thread(functools.partial(self._executor.shutdown, wait=True))

    async def _run_acquire(self, acquire: Callable[..., AcquireReturnProxy]) -> None:
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

    def _backend_holds(self) -> bool:
        # A sync close() or forced release on a shared singleton ends the tasks' level without the table taking part.
        return self._lock._holds_level_of(_TASKS_THREAD_ID)  # ruff:ignore[private-member-access]  # the wrapper owns this backend hold

    def _leave_backend(self) -> None:
        # Only the tasks' level: a sync user of the same singleton keeps its own.
        self._lock._release_level_of(_TASKS_THREAD_ID)  # ruff:ignore[private-member-access]  # the wrapper owns this backend hold

    def _submit(
        self, func: Callable[_P, _R], *args: _P.args, **kwargs: _P.kwargs
    ) -> asyncio.Future[_BackendOutcome[_R]]:
        return (self._loop or asyncio.get_running_loop()).run_in_executor(
            self._executor,
            _capture_call,
            functools.partial(func, *args, **kwargs),
        )

    def _shutdown_owned_executor(self) -> None:
        if self._owns_executor:
            self._executor.shutdown(wait=False)

    @property
    def _inherited(self) -> bool:
        return self._fork_invalidated or os.getpid() != self._creator_pid

    def _raise_if_unusable(self) -> None:
        _ensure_current_process()
        if self._inherited:  # pragma: needs fork
            msg = f"AsyncReadWriteLock on {self.lock_file} was invalidated by fork(); construct a new instance"
            raise RuntimeError(msg)
        if self._closed:
            msg = "Cannot operate on a closed database."
            raise sqlite3.ProgrammingError(msg)

    def _reset_after_fork_in_child(self) -> None:  # pragma: forked child
        self._fork_invalidated = True
        if self._owns_executor:
            # The worker did not survive the fork, yet before 3.12 the child's exit still wakes it through its queue. On
            # PyPy that queue is pure Python, and its lock may have been held by the worker at fork time, so exit hangs.
            # CPython 3.12+ forgets every inherited worker in the child; do the same for the one this lock owns.
            # typeshed declares the registry read-only; it is a WeakKeyDictionary the exit hook iterates.
            registry = cast(
                "MutableMapping[threading.Thread, object]",
                executor_threads._threads_queues,  # ruff:ignore[private-member-access]  # the exit hook's registry
            )
            for worker in cast("ThreadPoolExecutor", self._executor)._threads:  # ruff:ignore[private-member-access]  # only the executor lists its workers
                registry.pop(worker, None)

    def __del__(self) -> None:
        # Safety net when close() was never called: shut down the executor we own so its worker thread does not
        # outlive the lock. shutdown(wait=False) never blocks.
        if os.getpid() == getattr(self, "_creator_pid", None) and getattr(self, "_owns_executor", False):
            self._executor.shutdown(wait=False)


class AsyncAcquireReadWriteReturnProxy:
    """Context-aware object that releases the async read/write lock on exit."""

    def __init__(self, lock: AsyncReadWriteLock) -> None:
        self.lock = lock

    async def __aenter__(self) -> AsyncReadWriteLock:
        return self.lock

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.lock.release()


__all__ = [
    "AsyncAcquireReadWriteReturnProxy",
    "AsyncReadWriteLock",
]
