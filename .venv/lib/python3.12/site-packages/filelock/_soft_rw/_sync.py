"""Cross-process and cross-host reader/writer lock over a generation log of immutable snapshots."""

from __future__ import annotations

__lazy_modules__ = [
    "filelock._error",
    "filelock._lease",
    "filelock._soft_rw._protocol",
    "filelock._soft_rw._storage",
    "math",
]

import atexit
import os
import threading
import time
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from math import isfinite
from typing import TYPE_CHECKING, ClassVar, Final
from weakref import WeakValueDictionary

from filelock._api import (
    AcquireReturnProxy,
    _acquire_thread_lock,
    _canonical,
    _check_blocking,
    _check_timeout_max,
    _ensure_current_process,
    _register_fork_class,
    _register_fork_object,
    _resolve_read_write_timeout,
    _seconds,
    _timeout_behavior,
)
from filelock._error import Timeout
from filelock._lease import LeaseCompromise
from filelock._soft_rw._protocol import GenerationLog, Ledger, Mode, Participant
from filelock._soft_rw._storage import OsFiles

if TYPE_CHECKING:
    from collections.abc import Callable, Generator

    from filelock._lease import CompromiseReason

_ALL_INSTANCES: Final[WeakValueDictionary[int, SoftReadWriteLock]] = WeakValueDictionary()
_ALL_INSTANCES_LOCK: threading.Lock = threading.Lock()


class _SoftRWMeta(type):
    _instances: WeakValueDictionary[str, SoftReadWriteLock]
    _instances_lock: threading.RLock
    _instances_under_construction: set[str]

    def __call__(  # ruff:ignore[too-many-arguments]  # forwards the public constructor's documented parameters
        cls,
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        *,
        blocking: bool = True,
        is_singleton: bool = True,
        heartbeat_interval: float = 30.0,
        stale_threshold: float | None = None,
        poll_interval: float = 0.25,
        on_compromise: Callable[[LeaseCompromise], None] | None = None,
    ) -> SoftReadWriteLock:
        _ensure_current_process()
        # Reject a bad default at construction, as FileLock does, rather than at its first acquire.
        _check_blocking(blocking)
        timeout = _seconds("timeout", timeout)
        # Passed through only when set, so a subclass that declares its own constructor without it keeps working.
        extra = {} if on_compromise is None else {"on_compromise": on_compromise}
        if not is_singleton:
            return super().__call__(
                lock_file,
                timeout,
                blocking=blocking,
                is_singleton=is_singleton,
                heartbeat_interval=heartbeat_interval,
                stale_threshold=stale_threshold,
                poll_interval=poll_interval,
                **extra,
            )

        # Keyed like the log root, so one instance never spans two logs.
        normalized = _canonical(lock_file)
        with cls._instances_lock:
            instance = cls._instances.get(normalized)
            if instance is None:
                if normalized in cls._instances_under_construction:
                    msg = f"Singleton lock construction is already active for {lock_file!s}"
                    raise RuntimeError(msg)
                construction_pid = os.getpid()
                construction_registry = cls._instances_under_construction
                construction_registry.add(normalized)
                try:
                    instance = super().__call__(
                        lock_file,
                        timeout,
                        blocking=blocking,
                        is_singleton=is_singleton,
                        heartbeat_interval=heartbeat_interval,
                        stale_threshold=stale_threshold,
                        poll_interval=poll_interval,
                        **extra,
                    )
                finally:
                    construction_registry.discard(normalized)
                if os.getpid() != construction_pid:  # pragma: needs fork
                    msg = "Lock construction cannot continue after fork; construct a new lock in the child"
                    raise RuntimeError(msg)
                cls._instances[normalized] = instance
            elif (
                _timeout_behavior(instance.timeout, any_negative=False)
                != _timeout_behavior(timeout, any_negative=False)
                or instance.blocking != blocking
            ):
                msg = (
                    f"Singleton lock created with timeout={instance.timeout}, blocking={instance.blocking},"
                    f" cannot be changed to timeout={timeout}, blocking={blocking}"
                )
                raise ValueError(msg)
            elif on_compromise is not None and instance._on_compromise != on_compromise:  # ruff: ignore[private-member-access]  # the metaclass owns the singleton it compares
                # A tuning difference keeps the first caller's values; a different loss callback is a different safety
                # contract, and returning the cached instance would silently drop it.
                msg = f"Singleton lock created with a different on_compromise callback for {lock_file!s}"
                raise ValueError(msg)
            else:
                _validate_intervals(heartbeat_interval, stale_threshold, poll_interval)
            return instance


class SoftReadWriteLock(metaclass=_SoftRWMeta):
    """
    Cross-process and cross-host reader/writer lock for shared filesystems.

    Use this class instead of :class:`~filelock.ReadWriteLock` when the lock file lives on a network filesystem (NFS,
    Lustre, HPC cluster shared storage). ``ReadWriteLock`` is backed by SQLite and cannot run on NFS because SQLite's
    ``fcntl`` locking is unreliable there.

    The lock's state is a log of immutable snapshots under ``foo.lock.rw/gen/<N>``, each naming the writer and the
    readers holding the lock at generation ``N``. Every acquire, release, and eviction publishes the next snapshot with
    one atomic no-replace hard link, so no transition can be left half done by a crash on any host. Each participant
    also keeps ``foo.lock.rw/holders/<token>``, a record whose nonce a daemon heartbeat thread rewrites every
    ``heartbeat_interval`` seconds. A contender evicts a member whose record has not changed for ``stale_threshold``
    seconds of the contender's own monotonic clock; no clock is ever compared across hosts, so skew between hosts or
    against the file server cannot make a live holder look dead.

    Writer acquire is writer-preferring: a writer enters the snapshot as soon as no live writer is named, which blocks
    any new reader, then waits for the named readers to leave. Writer starvation is impossible.

    An expired holder is not fenced by the lock: a process paused past ``stale_threshold`` resumes believing it holds
    the lock until its next heartbeat reports the loss through ``on_compromise``. :attr:`generation` is a monotonic
    fencing token for the protected resource: a store that rejects writes carrying a lower generation than the highest
    it has accepted refuses such a holder.

    Reentrancy, upgrade/downgrade rules, thread pinning, and singleton caching by canonical path match
    :class:`~filelock.ReadWriteLock`.

    Forking invalidates the inherited instance in the child so the child cannot double-own the lock with its parent;
    ``release()`` on that instance is a no-op, and the child must construct a new instance if it needs a lock.

    Trust boundary: protects against same-UID non-cooperating processes (one host or cross-host) and same-host
    different-UID users via ``0o600`` / ``0o700`` permissions. Does not protect against root compromise or multi-tenant
    mounts where hostile co-tenants share the UID.

    :param lock_file: path to the lock file; the protocol directory lives next to it as ``<lock_file>.rw``
    :param timeout: maximum wait time in seconds; ``-1`` means block indefinitely
    :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately on contention
    :param is_singleton: if ``True``, reuse existing instances for the same canonical path
    :param heartbeat_interval: seconds between heartbeat refreshes; default 30 s
    :param stale_threshold: seconds a holder record may stay unchanged before a contender evicts it; defaults to
        ``3 * heartbeat_interval``, matching etcd's ``LeaseKeepAlive`` convention
    :param poll_interval: seconds between acquire retries under contention; default 0.25 s
    :param on_compromise: called from the heartbeat thread with a :class:`~filelock.LeaseCompromise` when the hold is
        lost: a peer evicted it, or refreshes failed for long enough that a peer could have

    .. versionadded:: 3.27.0

    .. versionchanged:: 3.33.0

        The on-disk protocol is a generation log; earlier releases cannot share a lock path with this one.

    """

    _instances: ClassVar[WeakValueDictionary[str, SoftReadWriteLock]] = WeakValueDictionary()
    _instances_lock = threading.RLock()
    _instances_under_construction: ClassVar[set[str]] = set()

    def __init_subclass__(cls, **kwargs: object) -> None:
        """Give each lock subclass its own singleton registry and lock."""
        super().__init_subclass__(**kwargs)
        cls._instances = WeakValueDictionary()
        cls._instances_lock = threading.RLock()
        cls._instances_under_construction = set()
        _register_fork_class(cls)

    def __init__(  # ruff:ignore[too-many-arguments]  # public constructor: one parameter per documented lock option
        self,
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        *,
        blocking: bool = True,
        is_singleton: bool = True,  # ruff:ignore[unused-method-argument]  # consumed by _SoftRWMeta.__call__
        heartbeat_interval: float = 30.0,
        stale_threshold: float | None = None,
        poll_interval: float = 0.25,
        on_compromise: Callable[[LeaseCompromise], None] | None = None,
    ) -> None:
        self._creator_pid = os.getpid()
        heartbeat_interval, stale_threshold, poll_interval = _validate_intervals(
            heartbeat_interval, stale_threshold, poll_interval
        )

        self.lock_file: str = os.fspath(lock_file)
        self.timeout: float = timeout
        self.blocking: bool = blocking
        self.heartbeat_interval: float = heartbeat_interval
        self.stale_threshold: float = stale_threshold
        self.poll_interval: float = poll_interval

        # Resolved once: a relative lock path must keep naming the same log after the process changes directory. The
        # final component stays unresolved, as in 4.0.12, so every release shares one log on a symlinked lock path.
        self._root = f"{_canonical(self.lock_file)}.rw"
        self._files = OsFiles(self.lock_file)
        self._ledger = Ledger(time.monotonic)
        self._log = GenerationLog(self._files, self.lock_file, self._root)
        self._on_compromise = on_compromise
        self._locks = _Locks(internal=threading.Lock(), transaction=threading.Lock())
        self._hold: _Hold | None = None
        self._compromise: LeaseCompromise | None = None
        self._closed: bool = False

        with _ALL_INSTANCES_LOCK:
            _ALL_INSTANCES[id(self)] = self
        _register_fork_object(self)

    @classmethod
    def _reset_class_after_fork(cls) -> None:  # pragma: forked child
        global _ALL_INSTANCES_LOCK  # ruff:ignore[global-statement]  # rebinds the module lock to a fresh one in the fork child
        _ALL_INSTANCES_LOCK = threading.Lock()
        cls._instances = WeakValueDictionary()
        cls._instances_lock = threading.RLock()
        cls._instances_under_construction = set()

    @property
    def generation(self) -> int | None:
        """
        The generation at which the current hold was granted, or ``None`` when no lock is held.

        Generations are monotonic across all participants and hosts, so this is a fencing token: pass it to the
        protected resource and have the resource reject any operation carrying a lower generation than the highest it
        has accepted. That refuses a holder that paused past ``stale_threshold``, was evicted, and resumed. Removing the
        ``<lock_file>.rw`` directory starts a new log whose generations begin again at 1.

        .. versionadded:: 3.33.0

        """
        with self._locks.internal:
            return None if self._hold is None else self._hold.participant.generation

    @property
    def compromise(self) -> LeaseCompromise | None:
        """
        How the current hold was lost, or ``None`` while it stands.

        Set by the heartbeat thread once a peer has evicted this holder or refreshes have failed for long enough that a
        peer could have. The holder should stop using the protected resource when it is set. Until the lost hold is
        released, acquiring on this instance raises :class:`RuntimeError` instead of joining it.

        .. versionadded:: 3.33.0

        """
        with self._locks.internal:
            return self._compromise

    @contextmanager
    def read_lock(self, timeout: float | None = None, *, blocking: bool | None = None) -> Generator[None]:
        """
        Context manager that acquires and releases a shared read lock.

        Falls back to instance defaults for *timeout* and *blocking* when ``None``.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

        :raises RuntimeError: if a write lock is already held on this instance
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        self.acquire_read(timeout, blocking=blocking)
        try:
            yield
        finally:
            self.release()

    @contextmanager
    def write_lock(self, timeout: float | None = None, *, blocking: bool | None = None) -> Generator[None]:
        """
        Context manager that acquires and releases an exclusive write lock.

        Falls back to instance defaults for *timeout* and *blocking* when ``None``.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

        :raises RuntimeError: if a read lock is already held, or a write lock is held by a different thread
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        self.acquire_write(timeout, blocking=blocking)
        try:
            yield
        finally:
            self.release()

    def acquire_read(self, timeout: float | None = None, *, blocking: bool | None = None) -> AcquireReturnProxy:
        """
        Acquire a shared read lock.

        If this instance already holds a read lock, the lock level is incremented (reentrant). Attempting to acquire a
        read lock while holding a write lock raises :class:`RuntimeError` (downgrade not allowed). On the 0→1
        transition the reader publishes its holder record, enters the next snapshot once no live writer is named, and
        starts a daemon heartbeat thread that rewrites the record's nonce every ``heartbeat_interval`` seconds so peers
        on other hosts do not evict it.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default; ``-1`` means block
            indefinitely
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately when the lock is unavailable;
            ``None`` uses the instance default

        :returns: a proxy that can be used as a context manager to release the lock

        :raises RuntimeError: if a write lock is already held on this instance, if this instance was invalidated by
            :func:`os.fork`, or if :meth:`close` was called
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``
        :raises SoftFileLockProtocolError: if a snapshot cannot be read without risking overlap, or the filesystem
            refuses the no-replace hard links the protocol commits with

        """
        return self._acquire("read", timeout, blocking=blocking)

    def acquire_write(self, timeout: float | None = None, *, blocking: bool | None = None) -> AcquireReturnProxy:
        """
        Acquire an exclusive write lock.

        If this instance already holds a write lock from the same thread, the lock level is incremented (reentrant).
        Attempting to acquire a write lock while holding a read lock raises :class:`RuntimeError` (upgrade not
        allowed). Write locks are pinned to the acquiring thread: a different thread trying to re-enter also raises
        :class:`RuntimeError`.

        A writer enters the next snapshot as its writer as soon as no live writer is named, which blocks every new
        reader on every host, then waits for the readers that snapshot names to leave. Writer starvation is impossible:
        new readers see the named writer and wait behind it.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default; ``-1`` means block
            indefinitely
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately when the lock is unavailable;
            ``None`` uses the instance default

        :returns: a proxy that can be used as a context manager to release the lock

        :raises RuntimeError: if a read lock is already held, if a write lock is held by a different thread, if this
            instance was invalidated by :func:`os.fork`, or if :meth:`close` was called
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``
        :raises SoftFileLockProtocolError: if a snapshot cannot be read without risking overlap, or the filesystem
            refuses the no-replace hard links the protocol commits with

        """
        return self._acquire("write", timeout, blocking=blocking)

    @classmethod
    def get_lock(
        cls,
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        *,
        blocking: bool = True,
    ) -> SoftReadWriteLock:
        """
        Return the singleton :class:`SoftReadWriteLock` for *lock_file*.

        :param lock_file: path to the lock file; the protocol directory lives next to it as ``<lock_file>.rw``
        :param timeout: maximum wait time in seconds; ``-1`` means block indefinitely
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately when the lock is unavailable

        :returns: the singleton lock instance

        :raises ValueError: if an instance already exists for this path with different *timeout* or *blocking* values

        """
        return cls(lock_file, timeout, blocking=blocking)

    def close(self) -> None:
        """
        Release any held lock and mark the instance closed.

        Idempotent. After calling this method the instance can no longer acquire locks — subsequent acquires raise
        :class:`RuntimeError`, and so does an acquire another thread is still waiting in, which gives up its claim
        instead of taking the lock. A fork-invalidated instance is closed without raising.
        """
        if self._creator_pid != os.getpid():  # pragma: forked child
            return
        # Closed first, under the lock an acquire publishes its hold under: an acquire still waiting then sees the flag
        # and leaves instead of publishing, and one that published before it is released below.
        with self._locks.internal:
            self._closed = True
        self.release(force=True)

    def release(self, *, force: bool = False) -> None:
        """
        Release one level of the current lock.

        When the lock level reaches zero the heartbeat thread is stopped, the participant is committed out of the next
        snapshot, and its holder record is removed. On a fork-invalidated instance (that is, the child of a
        :func:`os.fork` call made while the parent held a lock) this method is a no-op so inherited ``with`` blocks can
        unwind cleanly in the child.

        :param force: if ``True``, release the lock completely regardless of the current lock level

        :raises RuntimeError: if no lock is currently held and *force* is ``False``, or if the calling thread holds no
            level of the current lock and *force* is ``False``

        """
        if self._creator_pid != os.getpid():  # pragma: forked child
            return
        self._release(threading.get_ident(), force=force)

    def _acquire_level_of(
        self, holder: int, mode: Mode, timeout: float, *, blocking: bool, cancel_check: Callable[[], bool]
    ) -> AcquireReturnProxy:
        # Records the level under *holder* instead of the calling thread: an async wrapper's executor thread does not
        # identify the tasks it holds for. Gives up with Timeout once *cancel_check* is true.
        return self._acquire(mode, timeout, blocking=blocking, holder=holder, cancel_check=cancel_check)

    def _release_level_of(self, holder: int) -> None:
        self._release(holder, force=False)

    def _holds_level_of(self, holder: int) -> bool:
        # Read without the internal lock: an async wrapper asks from its event loop, which must not wait out a release
        # running on another thread.
        return (hold := self._hold) is not None and (holder == hold.write_thread_id or holder in hold.read_depths)

    def _release(self, thread_id: int, *, force: bool) -> None:
        with self._locks.internal:
            hold = self._hold
            if hold is None:
                if force:
                    return
                msg = f"Cannot release a lock on {self.lock_file} (lock id: {id(self)}) that is not held"
                raise RuntimeError(msg)
            if force:
                hold.level = 0
            else:
                self._raise_if_not_holder(hold, thread_id)
                hold.leave_one_level(thread_id)
            if hold.level > 0:
                return
            self._hold = None

        # Stop the heartbeat before leaving, so a late tick cannot re-read a snapshot this participant just left and
        # report an eviction that was its own release. on_compromise runs on that thread and may be what called here,
        # in which case there is nothing to join.
        hold.heartbeat_stop.set()
        try:
            if hold.heartbeat_thread is not threading.current_thread():
                hold.heartbeat_thread.join(timeout=min(self.heartbeat_interval + 1.0, threading.TIMEOUT_MAX))
        finally:
            # The hold is already gone from this instance, so a join that raises must not strand its record too.
            hold.participant.leave()

    def _raise_if_not_holder(self, hold: _Hold, thread_id: int) -> None:
        # The hold's own heartbeat thread may release it, since on_compromise runs there.
        if thread_id == hold.heartbeat_thread.ident:
            return
        # Another thread's release would leave the generation log under the holder and let a peer in.
        if hold.mode == "write" and thread_id != hold.write_thread_id:
            msg = (
                f"Cannot release write lock on {self.lock_file} (lock id: {id(self)}) "
                f"from thread {thread_id} while it is held by thread {hold.write_thread_id}"
            )
            raise RuntimeError(msg)
        # Readers share one participant, so a thread that holds no read level would end another's hold.
        if hold.mode == "read" and thread_id not in hold.read_depths:
            msg = (
                f"Cannot release read lock on {self.lock_file} (lock id: {id(self)}) "
                f"from thread {thread_id}, which does not hold it"
            )
            raise RuntimeError(msg)

    def _acquire(
        self,
        mode: Mode,
        timeout: float | None,
        *,
        blocking: bool | None,
        holder: int | None = None,
        cancel_check: Callable[[], bool] | None = None,
    ) -> AcquireReturnProxy:
        if self._creator_pid != os.getpid():  # pragma: forked child
            msg = f"SoftReadWriteLock on {self.lock_file} was invalidated by fork(); construct a new instance"
            raise RuntimeError(msg)
        timeout = self.timeout if timeout is None else timeout
        blocking = self.blocking if blocking is None else blocking
        holder = threading.get_ident() if holder is None else holder

        with self._locks.internal:
            if self._closed:
                raise self._closed_error()
            timeout = _resolve_read_write_timeout(timeout, blocking=blocking)
            if self._hold is not None:
                return self._validate_reentrant(mode, holder)

        deadline = None if timeout == -1 else time.perf_counter() + timeout
        if not _acquire_thread_lock(
            self._locks.transaction,
            blocking=blocking,
            timeout=timeout,
            poll_interval=self.poll_interval,
            cancel_check=cancel_check,
        ):
            raise Timeout(self.lock_file) from None
        try:
            return self._do_acquire_inner(mode, deadline, holder, blocking=blocking, cancel_check=cancel_check)
        finally:
            self._locks.transaction.release()

    def _closed_error(self) -> RuntimeError:
        return RuntimeError(f"SoftReadWriteLock on {self.lock_file} has been closed")

    def _do_acquire_inner(
        self,
        mode: Mode,
        deadline: float | None,
        holder: int,
        *,
        blocking: bool,
        cancel_check: Callable[[], bool] | None,
    ) -> AcquireReturnProxy:
        with self._locks.internal:
            if self._hold is not None:
                return self._validate_reentrant(mode, holder)
        participant = Participant(
            self._files,
            self._root,
            mode,
            stale_threshold=self.stale_threshold,
            clock=time.monotonic,
            ledger=self._ledger,
            log=self._log,
        )
        participant.publish()
        try:
            self._wait_for(participant.advance, deadline=deadline, blocking=blocking, cancel_check=cancel_check)
        except BaseException:
            # A contender that gives up must not stay named in the snapshot: a writer left there would block every
            # reader until a peer waits out the stale threshold. Its own token is all leave() touches, so it cannot
            # undo a peer's claim.
            with suppress(OSError):
                participant.leave()
            raise
        stop_event = threading.Event()
        hold = _Hold(
            level=1,
            mode=mode,
            write_thread_id=holder if mode == "write" else None,
            read_depths={holder: 1} if mode == "read" else {},
            participant=participant,
            heartbeat_thread=_HeartbeatThread(
                refresh=lambda: self._refresh(participant, hold),
                interval=self.heartbeat_interval,
                stop_event=stop_event,
                name=f"filelock-heartbeat-{id(self):x}",
            ),
            heartbeat_stop=stop_event,
            last_refresh=time.monotonic(),
        )
        # Publish the hold and start its heartbeat under one internal-lock section, so a concurrent release() never
        # observes a hold whose thread has not started and joins it. If the OS refuses the thread, clear the hold and
        # leave: left in place, a peer evicts the unrefreshed record and acquires while this instance still believes it
        # holds the lock. A close() that landed while this acquire waited gets the same treatment: published, the hold
        # would keep its heartbeat and its slot on an instance nobody can release through.
        start_error: BaseException | None = None
        with self._locks.internal:
            if self._closed:
                start_error = self._closed_error()
            else:
                self._hold = hold
                self._compromise = None
                try:
                    hold.heartbeat_thread.start()
                except BaseException as error:  # ruff:ignore[blind-except]  # clear the slot below and re-raise
                    self._hold = None
                    start_error = error
        if start_error is not None:
            participant.leave()
            raise start_error
        return AcquireReturnProxy(lock=self)

    def _validate_reentrant(self, mode: Mode, holder: int) -> AcquireReturnProxy:
        hold = self._hold
        assert hold is not None  # ruff:ignore[assert]  # callers dispatch here only inside the self._hold is not None branch
        if hold.mode != mode:
            opposite = "write" if mode == "read" else "read"
            direction = "downgrade" if mode == "read" else "upgrade"
            msg = (
                f"Cannot acquire {mode} lock on {self.lock_file} (lock id: {id(self)}): "
                f"already holding a {opposite} lock ({direction} not allowed)"
            )
            raise RuntimeError(msg)
        if mode == "write" and holder != hold.write_thread_id:
            msg = (
                f"Cannot acquire write lock on {self.lock_file} (lock id: {id(self)}) "
                f"from thread {holder} while it is held by thread {hold.write_thread_id}"
            )
            raise RuntimeError(msg)
        # Publishing a hold clears the loss recorded for the one before, so a loss seen here is this hold's. Its
        # heartbeat has stopped and a peer may hold the lock, so a new level must not bypass the log.
        if (compromise := self._compromise) is not None:
            raise _lost_hold_error(self.lock_file, mode, compromise)
        if mode == "read":
            hold.read_depths[holder] = hold.read_depths.get(holder, 0) + 1
        hold.level += 1
        return AcquireReturnProxy(lock=self)

    def _wait_for(
        self,
        predicate: Callable[[], bool],
        *,
        deadline: float | None,
        blocking: bool,
        cancel_check: Callable[[], bool] | None,
    ) -> None:
        while not predicate():
            # The flag is read without the lock here only to stop waiting early; publishing the hold re-checks it.
            if self._closed:
                raise self._closed_error()
            now = time.perf_counter()
            if not blocking or (deadline is not None and now >= deadline):
                raise Timeout(self.lock_file)
            # An abandoned acquire gives up like a timed-out one, so the caller's rollback withdraws its claim.
            if cancel_check is not None and cancel_check():
                raise Timeout(self.lock_file)
            sleep_for = self.poll_interval
            if deadline is not None:
                sleep_for = min(sleep_for, max(deadline - now, 0.0))
            time.sleep(sleep_for)

    def _refresh(self, participant: Participant, hold: _Hold) -> bool:
        # The loop ends at the first loss, so the holder hears about it once. A transient filesystem error (ESTALE /
        # EIO on the NFS-style filesystems this lock targets) is not a loss: retry rather than report a false
        # compromise. Report the record unrefreshable only once failures have run long enough that a peer could evict
        # it before the next success would land, a margin before the record actually ages out.
        try:
            outcome, error = participant.heartbeat()
        except Exception as failure:  # ruff:ignore[blind-except]  # the thread would end with the hold still published
            self._report_compromise(hold, "refresh-failed", failure)
            return False
        if outcome == "ok":
            hold.last_refresh = time.monotonic()
            return True
        if outcome == "lost":
            self._report_compromise(hold, "evicted", None)
            return False
        if time.monotonic() - hold.last_refresh >= self.stale_threshold - self.heartbeat_interval:
            self._report_compromise(hold, "refresh-failed", error)
            return False
        return True

    def _report_compromise(self, hold: _Hold, reason: CompromiseReason, error: Exception | None) -> None:
        # A tick that outlived its release's join must not stamp the old hold's loss onto whatever was acquired since.
        with self._locks.internal:
            if self._hold is not hold:
                return
            compromise = LeaseCompromise(
                lock_file=self.lock_file, token=hold.participant.token, reason=reason, error=error
            )
            self._compromise = compromise
        if self._on_compromise is not None:
            self._on_compromise(compromise)

    def _reset_after_fork_in_child(self) -> None:  # pragma: forked child
        self._locks = _Locks(internal=threading.Lock(), transaction=threading.Lock())
        self._hold = None


class _HeartbeatThread(threading.Thread):
    def __init__(
        self,
        refresh: Callable[[], bool],
        interval: float,
        stop_event: threading.Event,
        name: str,
    ) -> None:
        super().__init__(name=name, daemon=True)
        self._refresh = refresh
        self._interval = interval
        self._stop_event = stop_event

    def run(self) -> None:
        # Set on every exit, an on_compromise that raises included, so nothing waits on a thread that is gone.
        try:
            while not self._stop_event.wait(self._interval):
                if not self._refresh():
                    return
        finally:
            self._stop_event.set()


def _lost_hold_error(lock_file: str, mode: Mode, compromise: LeaseCompromise) -> RuntimeError:
    return RuntimeError(
        f"Cannot acquire {mode} lock on {lock_file}: the current hold was lost ({compromise.reason}), "
        "release it before acquiring again"
    )


def _validate_intervals(
    heartbeat_interval: float, stale_threshold: float | None, poll_interval: float
) -> tuple[float, float, float]:
    heartbeat_interval = _seconds("heartbeat_interval", heartbeat_interval)
    if not isfinite(heartbeat_interval) or heartbeat_interval <= 0:
        msg = f"heartbeat_interval must be positive and finite, got {heartbeat_interval}"
        raise ValueError(msg)
    # The heartbeat thread waits this long between refreshes, and Event.wait overflows past TIMEOUT_MAX.
    _check_timeout_max("heartbeat_interval", heartbeat_interval)
    stale_threshold = (
        heartbeat_interval * 3 if stale_threshold is None else _seconds("stale_threshold", stale_threshold)
    )
    if not isfinite(stale_threshold) or stale_threshold <= heartbeat_interval:
        msg = (
            f"stale_threshold must exceed heartbeat_interval ({heartbeat_interval}) "
            f"and be finite, got {stale_threshold}"
        )
        raise ValueError(msg)
    if not isfinite(poll_interval := _seconds("poll_interval", poll_interval)) or poll_interval <= 0:
        msg = f"poll_interval must be positive and finite, got {poll_interval}"
        raise ValueError(msg)
    _check_timeout_max("poll_interval", poll_interval)
    # A waiting writer refreshes its record once per poll, so a poll slower than the threshold reads as dead.
    if poll_interval >= stale_threshold:
        msg = f"poll_interval must be below stale_threshold ({stale_threshold}), got {poll_interval}"
        raise ValueError(msg)
    return heartbeat_interval, stale_threshold, poll_interval


@dataclass
class _Locks:
    internal: threading.Lock
    transaction: threading.Lock


@dataclass
class _Hold:
    """Everything that exists only while a lock is held; ``None`` when the instance has no lock."""

    level: int
    mode: Mode
    write_thread_id: int | None
    #: Read levels per thread, so a thread can give back only the levels it took.
    read_depths: dict[int, int]
    participant: Participant
    heartbeat_thread: _HeartbeatThread
    heartbeat_stop: threading.Event
    last_refresh: float

    def leave_one_level(self, thread_id: int) -> None:
        self.level -= 1
        if self.mode == "read":
            # The heartbeat thread holds no level of its own, so its release gives back one of a reader's.
            owner = thread_id if thread_id in self.read_depths else next(iter(self.read_depths))
            if depth := self.read_depths.pop(owner) - 1:
                self.read_depths[owner] = depth


def _cleanup_all_instances() -> None:  # pragma: no cover - runs from atexit at interpreter shutdown
    for instance in list(_ALL_INSTANCES.values()):
        with suppress(Exception):
            instance.release(force=True)


atexit.register(_cleanup_all_instances)
_register_fork_class(SoftReadWriteLock)


__all__ = [
    "SoftReadWriteLock",
    "_lost_hold_error",
]
