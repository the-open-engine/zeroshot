from __future__ import annotations

__lazy_modules__ = [
    "collections",
    "filelock._error",
    "filelock._identity",
    "hashlib",
    "itertools",
    "pathlib",
    "secrets",
    "tempfile",
]

import hashlib
import logging
import os
import pathlib
import secrets
import sqlite3
import stat
import sys
import tempfile
import threading
import time
from collections import deque
from contextlib import contextmanager, suppress
from dataclasses import dataclass, field
from itertools import count
from typing import TYPE_CHECKING, ClassVar, Final, Literal, NamedTuple, TypeAlias, cast
from weakref import WeakValueDictionary

from filelock._api import (
    AcquireReturnProxy,
    _acquire_thread_lock,
    _canonical,
    _check_blocking,
    _ensure_current_process,
    _fork_transition,
    _raise_chained_errors,
    _register_fork_class,
    _register_fork_object,
    _register_owned_descriptor,
    _resolve_lazy_imports_before_shutdown,
    _resolve_read_write_timeout,
    _seconds,
    _timeout_behavior,
    _unregister_owned_descriptor,
)
from filelock._error import Timeout
from filelock._identity import host_name, owner_is_stale, process_start_token

if sys.platform == "win32":  # pragma: win32 cover
    from filelock._windows import (
        _directory_is_private,
        _identity_at,
        _identity_of_fd,
        _make_private_directory,
        _open_non_reparse_fd,
        _pinned,
    )

if TYPE_CHECKING:
    from collections.abc import Callable, Generator
    from typing import Self

    from _typeshed import Unused

_LOGGER: Final[logging.Logger] = logging.getLogger("filelock")
_GETPID: Final[Callable[[], int]] = os.getpid
_IS_PYPY: Final[bool] = sys.implementation.name == "pypy"
_NEEDS_CONNECTION_ESCROW: Final[bool] = (
    hasattr(os, "register_at_fork") and sys.implementation.name == "cpython" and sys.version_info < (3, 12)
)
_ConnectionParameter: TypeAlias = (
    str | bytes | os.PathLike[str] | os.PathLike[bytes] | float | int | type[sqlite3.Connection] | None
)
_DatabaseIdentity: TypeAlias = tuple[int, int]

# sqlite3_busy_timeout() accepts a C int, max 2_147_483_647 on 32-bit. Use a lower value to be safe (~23 days).
_MAX_SQLITE_TIMEOUT_MS: Final[int] = 2_000_000_000 - 1
# How long a wait that its caller may abandon runs before it checks whether it should stop.
_CANCEL_POLL_INTERVAL: Final[float] = 0.1
_UNSAFE_FORK_EXIT_STATUS: Final[int] = 70
#: The longest a contended acquisition waits for the database inside one fork transition, in seconds.
_MAX_TRANSITION_WAIT: Final[float] = 0.05
# O_NONBLOCK keeps an open from blocking on a FIFO planted at the path; the regular-file check then rejects it.
_DB_OPEN_FLAGS: Final[int] = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
_DIRECTORY_FLAGS: Final[int] = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
# Header bytes 18 and 19 hold the file format versions, 2 in WAL mode: https://www.sqlite.org/fileformat.html
_SQLITE_MAGIC: Final[bytes] = b"SQLite format 3\x00"
_FORMAT_VERSIONS: Final[slice] = slice(18, 20)
_HEADER_SIZE: Final[int] = 20
_WAL_FORMAT_VERSION: Final[int] = 2


class _SQLiteTransitionContext(threading.local):
    depth: int = 0


_SQLITE_TRANSITION_CONTEXT: Final = _SQLiteTransitionContext()


class _ConnectionEscrow:
    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._functions: tuple[Callable[[sqlite3.Connection], None], Callable[[sqlite3.Connection], None]] | None = None

    def functions(
        self,
    ) -> tuple[Callable[[sqlite3.Connection], None], Callable[[sqlite3.Connection], None]] | None:
        if not _NEEDS_CONNECTION_ESCROW:
            return None  # pragma: >=3.12 cover
        with self._lock:  # pragma: <3.12 cover  # pragma: needs fork
            if self._functions is None:
                import ctypes  # ruff:ignore[import-outside-top-level]  # keep optional ctypes and its audited dlsym out of ordinary imports

                function_type = ctypes.PYFUNCTYPE(None, ctypes.py_object)
                increment_address = ctypes.cast(ctypes.pythonapi.Py_IncRef, ctypes.c_void_p).value
                decrement_address = ctypes.cast(ctypes.pythonapi.Py_DecRef, ctypes.c_void_p).value
                if increment_address is None or decrement_address is None:  # pragma: no cover - resolved CPython API
                    msg = "CPython reference functions have no address"
                    raise RuntimeError(msg)
                self._functions = (
                    cast("Callable[[sqlite3.Connection], None]", function_type(increment_address)),
                    cast("Callable[[sqlite3.Connection], None]", function_type(decrement_address)),
                )
            return self._functions

    def _reset_after_fork_in_child(self) -> None:  # pragma: forked child
        self._lock = threading.RLock()


_CONNECTION_ESCROW: Final = _ConnectionEscrow()


class _ForkedDatabaseRegistry:
    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._paths: set[pathlib.Path] = set()
        self._identities: set[_DatabaseIdentity] = set()
        self._sqlite_used = False
        self._all_paths_poisoned = False

    def raise_if_poisoned(self, path: pathlib.Path) -> None:
        identity = self.identity(path)
        with self._lock:
            all_paths_poisoned = self._all_paths_poisoned
            poisoned = (
                all_paths_poisoned or path in self._paths or (identity is not None and identity in self._identities)
            )
        if poisoned:  # pragma: needs fork
            msg = (
                "ReadWriteLock is unavailable in a PyPy fork child; exec or exit before using it"
                if all_paths_poisoned
                else f"SQLite database {path!s} was active across fork(); exec or exit before using it in the child"
            )
            raise RuntimeError(msg)

    def poison_after_fork(self, path: pathlib.Path, identity: _DatabaseIdentity | None) -> None:
        self._paths.add(path)
        if identity is not None:
            self._identities.add(identity)

    def note_sqlite_use(self) -> None:
        if _IS_PYPY:
            with self._lock:
                self._sqlite_used = True

    def _reset_after_fork_in_child(self) -> None:  # pragma: forked child
        self._lock = threading.RLock()
        self._all_paths_poisoned = self._all_paths_poisoned or (_IS_PYPY and self._sqlite_used)
        self._sqlite_used = False

    @staticmethod
    def identity(path: pathlib.Path) -> _DatabaseIdentity | None:
        try:
            stat_result = path.stat()
        except OSError:
            return None
        return stat_result.st_dev, stat_result.st_ino


_FORKED_DATABASES: Final = _ForkedDatabaseRegistry()

_LINK_PREFIX: Final[str] = ".filelock-"
_LINK_NAME: Final[str] = "lock.db"
_SWEPT_DATABASES: Final[set[_DatabaseIdentity]] = set()


# POSIX locks belong to the process and the inode, so closing any descriptor on the inode drops the locks every other
# connection of this process holds there. SQLite defers its own closes for this reason (section 2.2 of
# https://www.sqlite.org/howtocorrupt.html); the probes stay open until the last connection on their inode closes.
class _ProbeDescriptors:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._inodes: dict[_DatabaseIdentity, _InodeProbes] = {}
        self._releases: deque[_Probe] = deque()
        self._leases = count()

    def adopt(self, database: str, claimed: list[_Probe]) -> _Probe | None:  # pragma: win32 no cover
        # Reuse a probe no live connection holds: each connect would otherwise keep another descriptor open until the
        # inode's last close.
        try:
            stat_result: Final = os.lstat(database)
        except OSError:
            return None
        identity: Final = stat_result.st_dev, stat_result.st_ino
        with self._lock:
            if not stat.S_ISREG(stat_result.st_mode) or not (probes := self._inodes.get(identity)) or not probes.idle:
                return None
            claimed.append(adopted := _Probe(identity, probes.idle.pop(), next(self._leases)))
            probes.leases.add(adopted.lease)
        return adopted

    def retain(self, fd: int, claimed: list[_Probe]) -> _Probe:  # pragma: win32 no cover
        stat_result: Final = os.fstat(fd)
        identity: Final = stat_result.st_dev, stat_result.st_ino
        token: Final = _register_owned_descriptor(fd, identity)
        with self._lock:
            (probes := self._inodes.setdefault(identity, _InodeProbes())).descriptors.append((fd, token))
            claimed.append(retained := _Probe(identity, fd, next(self._leases)))
            probes.leases.add(retained.lease)
        return retained

    def release(self, probe: _Probe) -> None:  # pragma: win32 no cover
        # A finalizer may run this on a thread already inside the lock, so it never waits; the lock holder drains.
        self._releases.append(probe)
        self.drain()

    def drain(self) -> None:  # pragma: win32 no cover
        while self._releases and self._lock.acquire(blocking=False):
            try:
                # Dequeued only once applied, so an exception leaves the rest of the release for the next drain.
                while self._releases:
                    self._release_locked(self._releases[0])
                    self._releases.popleft()
            finally:
                self._lock.release()

    def _release_locked(self, probe: _Probe) -> None:  # pragma: win32 no cover
        if (probes := self._inodes.get(probe.identity)) is None:
            return
        if probe.lease in probes.leases:
            probes.leases.remove(probe.lease)
            if probes.leases:
                probes.idle.append(probe.fd)
        if probes.leases:
            return
        # No connect may adopt a probe about to close, even one an interrupt leaves listed.
        probes.idle.clear()
        # Closing under the lock keeps a concurrent connect from taking a lock on this inode before the close.
        while probes.descriptors:
            fd, token = probes.descriptors[-1]
            if token is not None:  # pragma: needs fork
                _unregister_owned_descriptor(token)
            # Forgotten before the close: after an interrupt, a leaked descriptor beats closing a reused number.
            probes.descriptors.pop()
            # The descriptor is gone even when close reports an error, and no lock of this process depends on it.
            with suppress(OSError):
                os.close(fd)
        del self._inodes[probe.identity]

    def _reset_after_fork_in_child(self) -> None:  # pragma: forked child
        # The child holds no POSIX locks, and the owned-descriptor registry closes the inherited probes.
        self._lock = threading.Lock()
        self._inodes = {}
        self._releases = deque()


class _Operation(NamedTuple):
    pid: int
    start_time: float
    #: Asked between bounded waits, so an abandoned acquire stops waiting; ``None`` waits in one call.
    cancel_check: Callable[[], bool] | None


class _Probe(NamedTuple):
    identity: _DatabaseIdentity
    fd: int
    # One per connection, so a release applied again after an interrupt kept it queued counts once.
    lease: int


@dataclass(slots=True)
class _InodeProbes:
    leases: set[int] = field(default_factory=set)
    descriptors: list[tuple[int, int | None]] = field(default_factory=list)
    idle: list[int] = field(default_factory=list)


_PROBE_DESCRIPTORS: Final = _ProbeDescriptors()


class _ForkSafeConnection(sqlite3.Connection):
    _creator_pid: int
    _decrement_escrow: Callable[[sqlite3.Connection], None] | None
    _database_directory: _LinkDirectory | None
    database_identity: _DatabaseIdentity | None = None
    _probe: _Probe | None = None

    def __new__(
        cls,
        *_args: _ConnectionParameter,
        **_kwargs: _ConnectionParameter,
    ) -> Self:
        connection = super().__new__(cls)
        connection._creator_pid = _GETPID()
        connection._decrement_escrow = None
        connection._database_directory = None
        return connection

    def close(self) -> None:
        with _sqlite_transition():
            if _GETPID() != self._creator_pid:  # pragma: needs fork
                return
            with _fork_transition():
                sqlite3.Connection.close(self)
                try:
                    if (directory := self._database_directory) is not None:  # pragma: needs posix-hard-link
                        self._database_directory = None
                        _remove_link_directory(directory)
                finally:
                    if (probe := self._probe) is not None:  # pragma: win32 no cover
                        self._probe = self.database_identity = None
                        _PROBE_DESCRIPTORS.release(probe)
                if (decrement := self._decrement_escrow) is not None:  # pragma: <3.12 cover  # pragma: needs fork
                    self._decrement_escrow = None
                    decrement(self)

    def retain_link(self, directory: _LinkDirectory | None) -> None:  # pragma: win32 cover
        """Keep the private link directory, if there is one, until SQLite no longer uses it."""
        self._database_directory = directory

    def retain_until_close(self, directory: _LinkDirectory | None, probe: _Probe) -> None:  # pragma: win32 no cover
        """Keep the private link and this connection's probe descriptor until SQLite no longer uses them."""
        self._database_directory = directory
        self._probe = probe
        self.database_identity = probe.identity

    def acquire_escrow(  # pragma: <3.12 cover  # pragma: needs fork
        self,
        functions: tuple[Callable[[sqlite3.Connection], None], Callable[[sqlite3.Connection], None]] | None,
    ) -> None:
        # The caller only reaches here holding the escrow functions; it skips the call entirely without them.
        if functions is not None:  # pragma: no branch
            increment, decrement = functions
            increment(self)
            self._decrement_escrow = decrement

    def __del__(self) -> None:
        with suppress(sqlite3.Error, RuntimeError, OSError):
            self.close()


@dataclass(slots=True)
class _OwnedConnection:
    """
    The connection of one lock, closed when the lock is collected.

    CPython runs ``__del__`` before it clears weak references. A finalizer on the lock itself would close the
    connection while the singleton cache still hands the lock out, and a thread given the lock would nest into a hold
    with no database lock behind it. The lock alone refers to this holder, so the holder is collected once the cache no
    longer finds the lock.
    """

    connection: _ForkSafeConnection | None = None

    def __del__(self) -> None:
        if (connection := self.connection) is not None:
            with suppress(sqlite3.Error, RuntimeError):
                connection.close()


class _ReadWriteLockMeta(type):
    """
    Resolve singleton instances for ``is_singleton=True`` construction.

    This logic lives here rather than in ReadWriteLock.get_lock so ``ReadWriteLock(path)`` returns cached instances
    without a 2-arg ``super()`` call that type checkers cannot verify.

    """

    _instances: WeakValueDictionary[pathlib.Path, ReadWriteLock]
    _instances_lock: threading.RLock
    _instances_pid: int
    _instances_under_construction: set[pathlib.Path]

    def __call__(
        cls,
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        *,
        blocking: bool = True,
        is_singleton: bool = True,
    ) -> ReadWriteLock:
        _ensure_current_process()
        _resolve_lazy_imports_before_shutdown()
        # Reject a bad default at construction, as FileLock does, rather than at its first acquire.
        _check_blocking(blocking)
        timeout = _seconds("timeout", timeout)
        if cls._instances_pid != _GETPID():
            cls._reset_class_after_fork()
        construction_pid = _GETPID()
        if not is_singleton:
            instance = super().__call__(lock_file, timeout, blocking=blocking, is_singleton=is_singleton)
            if _GETPID() != construction_pid:  # pragma: forked child
                msg = "ReadWriteLock construction cannot continue after fork"
                raise RuntimeError(msg)
            return instance

        # Resolve only the parent, so a symlink at the lock path stays its own key instead of aliasing its target.
        normalized = pathlib.Path(_canonical(lock_file))
        with cls._instances_lock:
            # One lookup, since another thread can drop the last reference between a membership check and a read.
            if (instance := cls._instances.get(normalized)) is None:
                if normalized in cls._instances_under_construction:  # pragma: no cover - exercised in an audit callback
                    msg = f"Singleton lock construction is already active for {lock_file!s}"
                    raise RuntimeError(msg)
                construction_registry = cls._instances_under_construction
                construction_registry.add(normalized)
                try:
                    instance = super().__call__(lock_file, timeout, blocking=blocking, is_singleton=is_singleton)
                finally:
                    if _GETPID() == construction_pid:
                        construction_registry.discard(normalized)
                if _GETPID() != construction_pid:
                    msg = "ReadWriteLock construction cannot continue after fork"
                    raise RuntimeError(msg)
                cls._instances[normalized] = instance

            if (
                _timeout_behavior(instance.timeout, any_negative=False)
                != _timeout_behavior(timeout, any_negative=False)
                or instance.blocking != blocking
            ):
                msg = (
                    f"Singleton lock created with timeout={instance.timeout}, blocking={instance.blocking},"
                    f" cannot be changed to timeout={timeout}, blocking={blocking}"
                )
                raise ValueError(msg)
            return instance

    def _reset_class_after_fork(cls) -> None:  # pragma: forked child
        cls._instances = WeakValueDictionary()
        cls._instances_lock = threading.RLock()
        cls._instances_pid = _GETPID()
        cls._instances_under_construction = set()


class ReadWriteLock(metaclass=_ReadWriteLockMeta):
    """
    Cross-process read-write lock backed by SQLite.

    Allows concurrent shared readers or a single exclusive writer. The lock is reentrant within the same mode (multiple
    ``acquire_read`` calls nest, as do multiple ``acquire_write`` calls from the same thread), but upgrading from read
    to write or downgrading from write to read raises :class:`RuntimeError`. Write locks are pinned to the thread that
    acquired them, and each thread sharing a read lock can release only the levels it acquired.

    An instance coordinates processes, not threads: a thread asking for the opposite mode of another thread's hold
    raises :class:`RuntimeError` instead of waiting. Threads that must wait for each other each need their own
    instance (``is_singleton=False``).

    By default, ``is_singleton=True``: calling ``ReadWriteLock(path)`` with the same resolved path returns the same
    instance. A ``.db`` extension is a convention rather than a requirement; the filesystem must be one the active
    SQLite VFS supports. Give the lock a file of its own: a WAL-mode database, or one with a schema larger than a lock
    needs, raises :class:`ValueError`. A database directory that other accounts can write must also be writable by
    this one and support hard links; the protection against other accounts ends once one of them can replace the
    database directory or a directory above it.

    :param lock_file: path to the SQLite database file used as the lock
    :param timeout: maximum wait time in seconds; ``-1`` waits up to SQLite's busy-timeout cap of about 23 days
    :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately when the lock is unavailable
    :param is_singleton: if ``True``, reuse existing instances for the same resolved path

    .. versionadded:: 3.21.0

    """

    _instances: ClassVar[WeakValueDictionary[pathlib.Path, ReadWriteLock]] = WeakValueDictionary()
    _instances_lock = threading.RLock()
    _instances_pid = _GETPID()
    _instances_under_construction: ClassVar[set[pathlib.Path]] = set()

    def __init_subclass__(cls) -> None:
        super().__init_subclass__()
        cls._instances = WeakValueDictionary()
        cls._instances_lock = threading.RLock()
        cls._instances_pid = _GETPID()
        cls._instances_under_construction = set()
        _register_fork_class(cls)

    @classmethod
    def get_lock(
        cls, lock_file: str | os.PathLike[str], timeout: float = -1, *, blocking: bool = True
    ) -> ReadWriteLock:
        """
        Return the singleton :class:`ReadWriteLock` for *lock_file*.

        :param lock_file: path to the SQLite database file used as the lock
        :param timeout: maximum wait time in seconds; ``-1`` waits up to SQLite's busy-timeout cap of about 23 days
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately when the lock is unavailable

        :returns: the singleton lock instance

        :raises ValueError: if an instance already exists for this path with different *timeout* or *blocking* values

        """
        return cls(lock_file, timeout, blocking=blocking)

    def __init__(
        self,
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        *,
        blocking: bool = True,
        is_singleton: bool = True,  # ruff:ignore[unused-method-argument]  # consumed by _ReadWriteLockMeta.__call__
    ) -> None:
        self.lock_file = os.fspath(lock_file)
        self._canonical_path = pathlib.Path(_canonical(lock_file))
        _FORKED_DATABASES.raise_if_poisoned(self._canonical_path)
        self.timeout = timeout
        self.blocking = blocking
        self._transaction_lock = threading.Lock()  # serializes the (possibly blocking) SQLite transaction work
        self._internal_lock = threading.Lock()  # protects _lock_level / _current_mode updates and rollback
        self._lock_level = 0
        self._current_mode: Literal["read", "write"] | None = None
        self._write_thread_id: int | None = None
        self._read_depths: dict[int, int] = {}
        self._acquisition_thread_ids: set[int] = set()
        self._owned_connection = _OwnedConnection()
        self._connection_transaction_released = True
        self._connection_identity: _DatabaseIdentity | None = None
        self._closed = False
        self._creator_pid = _GETPID()
        self._fork_invalidated = False
        _register_fork_object(self)
        with _fork_transition(), _sqlite_transition():
            validation_connection = self._open_connection(sqlite_timeout=5.0)
            validation_connection.close()

    def acquire_read(self, timeout: float | None = None, *, blocking: bool | None = None) -> AcquireReturnProxy:
        """
        Acquire a shared read lock.

        If this instance already holds a read lock, the lock level is incremented (reentrant). Attempting to acquire a
        read lock while holding a write lock raises :class:`RuntimeError` (downgrade not allowed).

        :param timeout: seconds to wait; ``None`` uses the instance setting; ``-1`` waits up to SQLite's
            busy-timeout cap of about 23 days
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` on contention;
            ``None`` uses the instance setting

        :returns: a proxy that can be used as a context manager to release the lock

        :raises RuntimeError: if a write lock is already held on this instance
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        return self._acquire(
            "read",
            self.timeout if timeout is None else timeout,
            blocking=self.blocking if blocking is None else blocking,
        )

    def acquire_write(self, timeout: float | None = None, *, blocking: bool | None = None) -> AcquireReturnProxy:
        """
        Acquire an exclusive write lock.

        If this instance already holds a write lock from the same thread, the lock level is incremented (reentrant).
        Attempting to acquire a write lock while holding a read lock raises :class:`RuntimeError` (upgrade not allowed).
        Write locks are pinned to the acquiring thread: a different thread trying to re-enter also raises
        :class:`RuntimeError`.

        :param timeout: seconds to wait; ``None`` uses the instance setting; ``-1`` waits up to SQLite's
            busy-timeout cap of about 23 days
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` on contention;
            ``None`` uses the instance setting

        :returns: a proxy that can be used as a context manager to release the lock

        :raises RuntimeError: if a read lock is already held, or a write lock is held by a different thread
        :raises Timeout: if the lock cannot be acquired within *timeout* seconds
        :raises TypeError: if *timeout* is a :class:`bool` or not a real number, or *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` *timeout* or a negative one other than ``-1``

        """
        return self._acquire(
            "write",
            self.timeout if timeout is None else timeout,
            blocking=self.blocking if blocking is None else blocking,
        )

    @contextmanager
    def read_lock(self, timeout: float | None = None, *, blocking: bool | None = None) -> Generator[None]:
        """
        Context manager that acquires and releases a shared read lock.

        Falls back to instance defaults for *timeout* and *blocking* when ``None``.

        :param timeout: maximum wait time in seconds, or ``None`` to use the instance default
        :param blocking: if ``False``, raise :class:`~filelock.Timeout` immediately; ``None`` uses the instance default

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

        """
        self.acquire_write(timeout, blocking=blocking)
        try:
            yield
        finally:
            self.release()

    def release(self, *, force: bool = False) -> None:
        """
        Release one level of the current lock.

        When the lock level reaches zero the underlying SQLite transaction is rolled back, releasing the database lock.

        :param force: if ``True``, release the lock completely regardless of the current lock level

        :raises RuntimeError: if no lock is currently held and *force* is ``False``, or if the calling thread holds
            no level of the current lock and *force* is ``False``

        """
        with _fork_transition():
            _ensure_current_process()
            if self._inherited:  # pragma: needs fork
                return
            self._raise_if_acquiring("release")
            self._release(force=force, close=False)

    def close(self) -> None:
        """
        Release the lock (if held) and close the underlying SQLite connection.

        After calling this method, the lock instance is no longer usable. Unlike :meth:`release`, it ends every
        thread's hold on this instance, whichever thread calls it.

        """
        with _fork_transition():
            _ensure_current_process()
            if self._inherited:  # pragma: needs fork
                return
            self._raise_if_acquiring("close")
            self._release(force=True, close=True)

    def _acquire_level_of(
        self,
        holder: int,
        mode: Literal["read", "write"],
        timeout: float,
        *,
        blocking: bool,
        cancel_check: Callable[[], bool],
    ) -> AcquireReturnProxy:
        # Records the level under *holder* instead of the calling thread: an async wrapper's executor thread does not
        # identify the tasks it holds for. Gives up with Timeout once *cancel_check* is true.
        return self._acquire(mode, timeout, blocking=blocking, holder=holder, cancel_check=cancel_check)

    def _release_level_of(self, thread_id: int) -> None:
        with _fork_transition():
            _ensure_current_process()
            self._raise_if_acquiring("release")
            self._release(force=False, close=False, thread_id=thread_id)

    def _holds_level_of(self, thread_id: int) -> bool:
        # Read without the internal lock. The async wrapper asks from its event loop, which must not wait out a release
        # on another thread. A stale answer is what a caller asking a moment earlier would have had. A level whose
        # transaction ended is one an interrupted release left behind; it counts as gone, as _holds_level() treats it.
        if thread_id != self._write_thread_id and thread_id not in self._read_depths:
            return False
        try:
            return not self._transaction_ended()
        except sqlite3.ProgrammingError:
            # The connection closed between the read of _con and the in_transaction call, so the transaction is over.
            return False

    def _release(self, *, force: bool, close: bool, thread_id: int | None = None) -> None:
        thread_id = threading.get_ident() if thread_id is None else thread_id
        with self._transaction_lock, self._internal_lock:
            if not force:
                self._raise_if_not_holder(thread_id)
            if self._lock_level == 0:
                if force and self._con is None:
                    if close:
                        self._closed = True
                    return
                if not force:
                    msg = f"Cannot release a lock on {self.lock_file} (lock id: {id(self)}) that is not held"
                    raise RuntimeError(msg)
            if not force and self._lock_level > 1:
                self._leave_one_level(thread_id)
                return
            try:
                self._finish_connection()
            except BaseException:
                # Once the rollback ran, the level is gone whatever stopped the cleanup, a signal handler's exception
                # included. Kept, it would let the next acquire nest into a transaction that no longer exists.
                if self._transaction_ended():
                    self._clear_lock_state()
                raise
            self._clear_lock_state()
            if close:
                self._closed = True

    def _transaction_ended(self) -> bool:
        if self._connection_transaction_released or (connection := self._con) is None:
            return True
        # An interrupt can land between the rollback and the flag that records it.
        return not connection.in_transaction

    def _raise_if_not_holder(self, thread_id: int) -> None:
        # The connection runs with check_same_thread=False, so a release from another thread would end the owner's
        # transaction under it and let a second writer in.
        if self._current_mode == "write" and thread_id != self._write_thread_id:
            msg = (
                f"Cannot release write lock on {self.lock_file} (lock id: {id(self)}) "
                f"from thread {thread_id} while it is held by thread {self._write_thread_id}"
            )
            raise RuntimeError(msg)
        # Readers share that one transaction too, so a thread that holds no read level would end another's hold.
        if self._current_mode == "read" and thread_id not in self._read_depths:
            msg = (
                f"Cannot release read lock on {self.lock_file} (lock id: {id(self)}) "
                f"from thread {thread_id}, which does not hold it"
            )
            raise RuntimeError(msg)

    def _leave_one_level(self, thread_id: int) -> None:
        self._lock_level -= 1
        if self._current_mode == "read" and (depth := self._read_depths.pop(thread_id) - 1):
            self._read_depths[thread_id] = depth

    def _clear_lock_state(self) -> None:
        self._lock_level = 0
        self._current_mode = None
        self._write_thread_id = None
        self._read_depths = {}

    @property
    def _con(self) -> _ForkSafeConnection | None:
        return self._owned_connection.connection

    @_con.setter
    def _con(self, connection: _ForkSafeConnection | None) -> None:
        self._owned_connection.connection = connection

    def _reset_after_fork_in_child(self) -> None:  # pragma: forked child
        if self._con is not None:
            _FORKED_DATABASES.poison_after_fork(self._canonical_path, self._connection_identity)
        self._con = None
        self._connection_transaction_released = True
        self._connection_identity = None
        self._transaction_lock = threading.Lock()
        self._internal_lock = threading.Lock()
        self._clear_lock_state()
        self._acquisition_thread_ids = set()
        self._fork_invalidated = True

    @property
    def _inherited(self) -> bool:
        return self._fork_invalidated or _GETPID() != self._creator_pid

    def _raise_if_unusable(self) -> None:
        _ensure_current_process()
        if self._inherited:  # pragma: needs fork
            msg = f"ReadWriteLock on {self.lock_file} was invalidated by fork(); construct a new instance"
            raise RuntimeError(msg)
        if self._closed:
            msg = "Cannot operate on a closed database."
            raise sqlite3.ProgrammingError(msg)

    def _acquire(
        self,
        mode: Literal["read", "write"],
        timeout: float,
        *,
        blocking: bool,
        holder: int | None = None,
        cancel_check: Callable[[], bool] | None = None,
    ) -> AcquireReturnProxy:
        timeout = _resolve_read_write_timeout(timeout, blocking=blocking)
        start_time: Final = time.monotonic()
        remaining = timeout  # -1 waits up to SQLite's busy-timeout cap
        while True:
            # A fork waits for the open transitions and admits no new one meanwhile. A wait for the database inside one
            # stalls the forking thread and, behind it, each lock call in the process. With no other thread to fork,
            # the wait runs in one piece, which keeps SQLite's writer preference. threading does not count a
            # thread started through _thread, so a fork from one of those still waits.
            final = (
                not blocking
                or 0 <= remaining <= _MAX_TRANSITION_WAIT
                or not hasattr(os, "register_at_fork")
                or threading.active_count() == 1
            )
            try:
                return self._attempt_acquire(
                    mode,
                    remaining if final else _MAX_TRANSITION_WAIT,
                    blocking=blocking,
                    holder=holder,
                    cancel_check=cancel_check,
                )
            except Timeout:
                if final or (cancel_check is not None and cancel_check()):
                    raise
            # Only a runtime with fork handlers waits in more than one attempt.
            if timeout != -1:  # pragma: needs register-at-fork
                remaining = max(timeout - (time.monotonic() - start_time), 0)

    def _attempt_acquire(
        self,
        mode: Literal["read", "write"],
        timeout: float,
        *,
        blocking: bool,
        holder: int | None = None,
        cancel_check: Callable[[], bool] | None = None,
    ) -> AcquireReturnProxy:
        with _fork_transition():
            self._raise_if_unusable()
            operation_pid = _GETPID()
            thread_id = threading.get_ident()
            holder = thread_id if holder is None else holder
            with self._internal_lock:
                if self._holds_level():
                    return self._validate_reentrant(mode, holder)
                if thread_id in self._acquisition_thread_ids:  # pragma: no cover - exercised in an audit callback
                    msg = f"Cannot acquire ReadWriteLock on {self.lock_file} while acquisition is active in this thread"
                    raise RuntimeError(msg)
                self._acquisition_thread_ids.add(thread_id)
            try:
                start_time = time.perf_counter()
                self._acquire_transaction_lock(blocking=blocking, timeout=timeout, cancel_check=cancel_check)
                try:
                    self._raise_if_unusable()
                    return self._do_acquire_inner(
                        mode,
                        timeout,
                        holder,
                        blocking=blocking,
                        operation=_Operation(operation_pid, start_time, cancel_check),
                    )
                finally:
                    self._transaction_lock.release()
            finally:
                with self._internal_lock:
                    self._acquisition_thread_ids.discard(thread_id)

    def _do_acquire_inner(
        self,
        mode: Literal["read", "write"],
        timeout: float,
        holder: int,
        *,
        blocking: bool,
        operation: _Operation,
    ) -> AcquireReturnProxy:
        operation_pid, start_time, _ = operation
        # Double-check: another thread may have acquired the lock while we waited on _transaction_lock.
        with self._internal_lock:
            if self._holds_level():
                return self._validate_reentrant(mode, holder)
        if self._con is not None:
            self._finish_connection()
        try:
            self._open_for_acquisition(
                timeout,
                blocking=blocking,
                operation_pid=operation_pid,
                start_time=start_time,
            )
            self._configure_and_begin(mode, timeout, blocking=blocking, operation=operation)
            self._raise_if_process_changed(operation_pid)
        except BaseException as error:
            acquisition_error: BaseException
            if isinstance(error, sqlite3.OperationalError) and "database is locked" in str(error):
                acquisition_error = Timeout(self.lock_file)
            else:
                acquisition_error = error
            try:
                self._finish_connection()
            except sqlite3.Error as cleanup_error:
                _raise_chained_errors(acquisition_error, cleanup_error)
            if acquisition_error is not error:
                raise acquisition_error from None
            raise
        with self._internal_lock:
            self._raise_if_process_changed(operation_pid)
            self._current_mode = mode
            self._lock_level = 1
            if mode == "write":
                self._write_thread_id = holder
            else:
                self._read_depths = {holder: 1}
        return AcquireReturnProxy(lock=self)

    def _open_for_acquisition(self, timeout: float, *, blocking: bool, operation_pid: int, start_time: float) -> None:
        with _sqlite_transition():
            sqlite_timeout = (
                timeout_for_sqlite(
                    timeout,
                    blocking=blocking,
                    already_waited=time.perf_counter() - start_time,
                )
                / 1000
            )
            connection = self._open_connection(sqlite_timeout=sqlite_timeout)
            self._con, self._connection_transaction_released, self._connection_identity = (
                connection,
                False,
                _FORKED_DATABASES.identity(self._canonical_path),
            )
            self._raise_if_process_changed(operation_pid)

    def _configure_and_begin(
        self,
        mode: Literal["read", "write"],
        timeout: float,
        *,
        blocking: bool,
        operation: _Operation,
    ) -> None:
        with _sqlite_transition():
            connection = cast("_ForkSafeConnection", self._con)
            # SQLite opened the file the name held at connect time, and its first statement would replay a journal
            # beside that file, so the name is checked before any statement runs.
            self._raise_if_database_replaced(connection)
            with _bounded_schema(connection, self.lock_file):
                self._raise_if_process_changed(operation.pid)
                # Use legacy journal mode (not WAL) because WAL does not block readers while a concurrent EXCLUSIVE
                # write transaction is active, which makes read-write locking impossible without modifying table data.
                # MEMORY is safe here since no writes happen, so a crash cannot corrupt the DB.
                # See https://sqlite.org/lang_transaction.html#deferred_immediate_and_exclusive_transactions
                self._execute_waiting(
                    connection, "PRAGMA journal_mode=MEMORY;", timeout, blocking=blocking, operation=operation
                )
                self._raise_if_process_changed(operation.pid)
                statements = "BEGIN EXCLUSIVE TRANSACTION;" if mode == "write" else "BEGIN TRANSACTION;"
                if mode == "read":
                    # SQLite takes the SHARED lock only when a statement reads; BEGIN alone stays deferred.
                    # https://www.sqlite.org/lockingv3.html#transaction_control
                    # sqlite_schema is an alias added in SQLite 3.33.0; sqlite_master works on every version.
                    statements += " SELECT name FROM sqlite_master LIMIT 1;"
                self._execute_waiting(connection, statements, timeout, blocking=blocking, operation=operation)
            self._raise_if_database_replaced(connection)

    @staticmethod
    def _execute_waiting(
        connection: _ForkSafeConnection, statements: str, timeout: float, *, blocking: bool, operation: _Operation
    ) -> None:
        # SQLite's busy wait cannot be interrupted. An acquire its caller may abandon waits in bounded slices and asks
        # between them whether to stop.
        slice_ms = int(_CANCEL_POLL_INTERVAL * 1000)
        while True:
            busy_ms = timeout_for_sqlite(
                timeout, blocking=blocking, already_waited=time.perf_counter() - operation.start_time
            )
            cancel_check = operation.cancel_check if busy_ms > slice_ms else None
            try:
                connection.executescript(
                    f"PRAGMA busy_timeout={busy_ms if cancel_check is None else slice_ms}; {statements}"
                ).close()
            except sqlite3.OperationalError as error:
                if cancel_check is None or "database is locked" not in str(error) or cancel_check():
                    raise
                # A read's BEGIN succeeds before its SELECT waits, so the retry has to start a fresh transaction.
                if connection.in_transaction:
                    connection.rollback()
            else:
                return

    def _raise_if_database_replaced(self, connection: _ForkSafeConnection) -> None:
        # Windows pins the name: SQLite opens the database without delete sharing, so it cannot be unlinked or renamed.
        if (identity := connection.database_identity) is None:  # pragma: win32 cover
            return
        try:  # pragma: win32 no cover
            current = self._canonical_path.stat(follow_symlinks=False)
        except FileNotFoundError:  # pragma: win32 no cover
            current = None
        # The lock lives on the inode, so one taken after the path moved on excludes no later opener.
        if current is None or (current.st_dev, current.st_ino) != identity:  # pragma: win32 no cover
            msg = f"lock database {self.lock_file!r} was unlinked or replaced while acquiring; leave it in place"
            raise OSError(msg)

    def _open_connection(self, *, sqlite_timeout: float) -> _ForkSafeConnection:
        with _sqlite_transition():
            creator_pid = _GETPID()
            functions = _CONNECTION_ESCROW.functions()
            if _GETPID() != creator_pid:  # pragma: forked child
                msg = "SQLite connection construction cannot continue after fork"
                raise RuntimeError(msg)
            try:
                connection = _connect(
                    os.fspath(self._canonical_path),
                    factory=_ForkSafeConnection,
                    timeout=sqlite_timeout,
                )
            except BlockingIOError:
                raise Timeout(self.lock_file) from None
            if functions is not None:  # pragma: <3.12 cover  # pragma: needs fork
                connection.acquire_escrow(functions)
            if _GETPID() != creator_pid:  # pragma: forked child
                _FORKED_DATABASES.poison_after_fork(
                    self._canonical_path,
                    _FORKED_DATABASES.identity(self._canonical_path),
                )
                msg = "SQLite connection construction cannot continue after fork"
                raise RuntimeError(msg)
            return connection

    def _finish_connection(self) -> None:
        with _sqlite_transition():
            if (connection := self._con) is None:
                return
            rollback_error: sqlite3.Error | None = None
            if not self._connection_transaction_released:
                if connection.in_transaction:
                    try:
                        connection.rollback()
                    except sqlite3.Error as error:
                        if connection.in_transaction:
                            raise
                        self._connection_transaction_released = True
                        rollback_error = error
                    else:
                        self._connection_transaction_released = True
                else:
                    self._connection_transaction_released = True
            try:
                connection.close()
            except sqlite3.Error as close_error:
                if rollback_error is not None:
                    _raise_chained_errors(rollback_error, close_error)
                raise
            self._con = None
            self._connection_transaction_released = True
            self._connection_identity = None
            if rollback_error is not None:
                raise rollback_error

    def _holds_level(self) -> bool:
        # A release interrupted after its rollback, but before it recorded that, leaves a level with no transaction.
        if self._lock_level > 0 and self._transaction_ended():
            self._clear_lock_state()
        return self._lock_level > 0

    def _validate_reentrant(self, mode: Literal["read", "write"], cur: int) -> AcquireReturnProxy:
        if self._current_mode != mode:
            opposite = "write" if mode == "read" else "read"
            if cur == self._write_thread_id or cur in self._read_depths:
                direction = "downgrade" if mode == "read" else "upgrade"
                reason = f"already holding a {opposite} lock ({direction} not allowed)"
            else:
                reason = (
                    f"another thread holds a {opposite} lock on this instance, which coordinates processes rather "
                    "than threads; give each thread its own instance with is_singleton=False"
                )
            msg = f"Cannot acquire {mode} lock on {self.lock_file} (lock id: {id(self)}): {reason}"
            raise RuntimeError(msg)
        if mode == "write" and cur != self._write_thread_id:
            msg = (
                f"Cannot acquire write lock on {self.lock_file} (lock id: {id(self)}) "
                f"from thread {cur} while it is held by thread {self._write_thread_id}"
            )
            raise RuntimeError(msg)
        if mode == "read":
            self._read_depths[cur] = self._read_depths.get(cur, 0) + 1
        self._lock_level += 1
        return AcquireReturnProxy(lock=self)

    def _acquire_transaction_lock(
        self, *, blocking: bool, timeout: float, cancel_check: Callable[[], bool] | None
    ) -> None:
        if not _acquire_thread_lock(
            self._transaction_lock,
            blocking=blocking,
            timeout=timeout,
            poll_interval=_CANCEL_POLL_INTERVAL,
            cancel_check=cancel_check,
        ):
            raise Timeout(self.lock_file) from None

    def _raise_if_acquiring(self, operation: Literal["acquire", "close", "release"]) -> None:
        with self._internal_lock:
            active_in_current_thread = threading.get_ident() in self._acquisition_thread_ids
        if active_in_current_thread:  # pragma: no cover - exercised in an audit callback
            msg = f"Cannot {operation} ReadWriteLock on {self.lock_file} while acquisition is active in this thread"
            raise RuntimeError(msg)

    def _raise_if_process_changed(self, operation_pid: int) -> None:
        if _GETPID() != operation_pid or self._inherited:  # pragma: forked child
            msg = f"ReadWriteLock on {self.lock_file} was invalidated by fork(); construct a new instance"
            raise RuntimeError(msg)


def _connect(database: str, *, factory: type[_ForkSafeConnection], timeout: float) -> _ForkSafeConnection:
    _FORKED_DATABASES.note_sqlite_use()
    # A symlink at the path would make SQLite open, lock, or create its target, so connect through a descriptor that
    # refuses one.
    if sys.platform == "win32":  # pragma: win32 cover
        fd: Final = _open_lock_database(database, oplock_wait=timeout)
        link: _LinkDirectory | None = None
        # A Windows lock belongs to the handle that took it, so closing this one leaves SQLite's locks in place.
        try:
            _raise_if_wal(fd, database)
            target = database
            if not _directory_is_private(os.fspath(pathlib.Path(database).parent)):
                link = _link_beside_on_windows(database, _identity_of_fd(fd))
                target = os.fspath(link.database)
            try:
                connection = sqlite3.connect(
                    target, check_same_thread=False, factory=factory, cached_statements=0, timeout=timeout
                )
            except BaseException:
                if link is not None:
                    _remove_link_directory(link)
                raise
            connection.retain_link(link)
            return connection
        finally:
            os.close(fd)
    return _connect_through_descriptor(  # pragma: win32 no cover
        database, factory=factory, timeout=timeout, owner=os.geteuid()
    )


def _connect_through_descriptor(  # pragma: win32 no cover
    database: str, *, factory: type[_ForkSafeConnection], timeout: float, owner: int
) -> _ForkSafeConnection:
    # Listed before its lease is taken and cleared once a connection owns it, so any exception between, an interrupt
    # included, releases the lease instead of pinning the inode's probes.
    claimed: Final[list[_Probe]] = []
    link: _LinkDirectory | None = None
    try:
        if (probe := _PROBE_DESCRIPTORS.adopt(database, claimed)) is None:
            probe = _PROBE_DESCRIPTORS.retain(_open_lock_database(database), claimed)
        identity, fd, _ = probe
        # Releases a finalizer queued while the table was locked.
        _PROBE_DESCRIPTORS.drain()
        _raise_if_wal(fd, database)
        target = database
        # SQLite opens the name it is given, following symlinks, and uses the -journal, -wal and -shm names beside it,
        # so the database name is safe only where no process of another account can create names. A descriptor path
        # such as /proc/self/fd/<n> is no better: SQLite reads it back into the name the kernel reports.
        if not _names_are_private(pathlib.Path(database).parent, owner):  # pragma: needs posix-hard-link
            link = _link_beside(database, identity, owner)
            target = os.fspath(link.database)
        connection = sqlite3.connect(
            target, check_same_thread=False, factory=factory, cached_statements=0, timeout=timeout
        )
        connection.retain_until_close(link, probe)
        claimed.clear()
        return connection
    finally:
        for unowned in claimed:
            _PROBE_DESCRIPTORS.release(unowned)
        if claimed and link is not None:  # pragma: needs posix-hard-link
            # A connection that already took the link removes it too when it closes.
            _remove_link_directory(link)


def _names_are_private(directory: pathlib.Path, owner: int) -> bool:  # pragma: win32 no cover
    status: Final = os.lstat(directory)
    # The sticky bit still lets others create names, so a directory anyone else may write to never qualifies.
    return status.st_uid in {owner, 0} and not status.st_mode & (stat.S_IWGRP | stat.S_IWOTH)


class _LinkDirectory(NamedTuple):  # pragma: needs posix-hard-link
    path: pathlib.Path
    identity: _DatabaseIdentity
    # Made through a descriptor of path, which nobody else can list, so a directory another process swaps in under
    # path cannot hold this name and SQLite fails to open instead of following the swap.
    name: str

    @property
    def database(self) -> pathlib.Path:
        return self.path / self.name / _LINK_NAME


def _link_beside(  # pragma: needs posix-hard-link
    database: str, identity: _DatabaseIdentity, owner: int
) -> _LinkDirectory:
    # Beside the database because a link cannot cross filesystems, and TMPDIR is often on another one.
    parent: Final = pathlib.Path(database).parent
    _sweep_links_of_the_dead(parent, identity, owner)
    path: Final = pathlib.Path(tempfile.mkdtemp(prefix=_link_prefix(), dir=parent))
    fd: Final = os.open(path, _DIRECTORY_FLAGS)
    try:
        made: Final = os.fstat(fd)
        # Whoever can rename in the parent can put another directory under the new name before it opens.
        loose: Final = made.st_mode & 0o077
        if made.st_uid != owner or (loose and os.listdir(fd)):  # ruff:ignore[os-listdir]  # pathlib cannot list a descriptor
            msg = f"link directory {os.fspath(path)!r} was replaced while opening"
            raise OSError(msg)
        # An inherited ACL, such as an NFSv4 one on ZFS, overrides the mode mkdtemp asks for. Only an empty directory
        # is tightened, so one swapped in with contents is still refused above.
        if loose:
            msg = (
                f"link directory {os.fspath(path)!r} stays open to other accounts through an inherited ACL"
                f" that chmod 0700 cannot close; make {os.fspath(parent)!r} writable by its owner only"
            )
            try:
                os.fchmod(fd, 0o700)
            except PermissionError as exc:  # ZFS with aclmode=restricted refuses a chmod on a non-trivial ACL
                raise OSError(msg) from exc
            if os.fstat(fd).st_mode & 0o077:
                raise OSError(msg)
        link: Final = _LinkDirectory(path, (made.st_dev, made.st_ino), secrets.token_hex(8))
        try:
            os.mkdir(link.name, 0o700, dir_fd=fd)
            os.link(database, link.database, follow_symlinks=False)
            linked: Final = link.database.stat(follow_symlinks=False)
        except BaseException:
            _remove_link_directory(link)
            raise
    finally:
        os.close(fd)
    if (linked.st_dev, linked.st_ino) != identity:
        _remove_link_directory(link)
        msg = f"lock database changed while opening: {database!r}"
        raise OSError(msg)
    return link


def _sweep_links_of_the_dead(
    parent: pathlib.Path, identity: _DatabaseIdentity, owner: int
) -> None:  # pragma: needs posix-hard-link
    # A process killed while it held a link leaves the directory, and an extra link to the database, behind for good.
    # Two threads sweeping at once only both try the same removals, which a lock would cost a fork-safety reset for.
    if identity in _SWEPT_DATABASES:
        return
    _SWEPT_DATABASES.add(identity)
    try:
        with os.scandir(parent) as entries:
            names: Final = [
                entry.name
                for entry in entries
                if entry.name.startswith(_LINK_PREFIX) and entry.is_dir(follow_symlinks=False)
            ]
    except OSError:
        return
    for name in names:
        # A peer sweeping alongside, or a directory this user cannot remove, must not fail the connect.
        with suppress(OSError):
            _remove_dead_links(parent / name, identity, owner)


def _remove_dead_links(
    path: pathlib.Path, identity: _DatabaseIdentity, owner: int
) -> None:  # pragma: needs posix-hard-link
    if sys.platform == "win32":  # pragma: win32 cover
        _remove_dead_links_on_windows(path, identity)
        return
    # Any directory can carry a dead holder's name after a rename in the parent, so only hard links to this database
    # go, and the directory only once that leaves it empty.
    fd: Final = os.open(path, _DIRECTORY_FLAGS)
    try:
        # Only a directory this account made carries a holder of its own; a name another account chose is never
        # parsed, so no PID is probed on its behalf.
        if os.fstat(fd).st_uid != owner or not _owner_is_gone(path.name):
            return
        with os.scandir(fd) as entries:
            names: Final = [entry.name for entry in entries]
        for entry in names:
            # Releases up to 4.2.0 linked the database right inside the directory.
            if entry == _LINK_NAME:
                _unlink_link_to(identity, fd)
                continue
            try:
                nested = os.open(entry, _DIRECTORY_FLAGS, dir_fd=fd)
            except OSError:
                continue
            try:
                _unlink_link_to(identity, nested)
            finally:
                os.close(nested)
            with suppress(OSError):
                os.rmdir(entry, dir_fd=fd)
    finally:
        os.close(fd)
    with suppress(OSError):
        path.rmdir()


def _unlink_link_to(identity: _DatabaseIdentity, dir_fd: int) -> None:  # pragma: needs posix-hard-link
    try:
        linked: Final = os.stat(_LINK_NAME, dir_fd=dir_fd, follow_symlinks=False)
    except OSError:
        return
    if (linked.st_dev, linked.st_ino) == identity:
        os.unlink(_LINK_NAME, dir_fd=dir_fd)


def _owner_is_gone(name: str) -> bool:  # pragma: needs posix-hard-link
    match name.removeprefix(_LINK_PREFIX).split("-"):
        case [pid, start, digest, _] if (
            pid.isascii() and pid.isdigit() and (not start or (start.isascii() and start.isdigit()))
        ):
            if not 1 <= (owner_pid := int(pid)) <= 2**31 - 1:
                return False
            host: Final = host_name()
            return digest == _host_digest(host) and owner_is_stale(owner_pid, host, int(start) if start else None)
        case _:
            return False


def _link_prefix() -> str:  # pragma: needs posix-hard-link
    # The owner rides in the name, which mkdtemp creates in one step, so no directory ever exists without it.
    start: Final = process_start_token(pid := os.getpid())
    return f"{_LINK_PREFIX}{pid}-{'' if start is None else start}-{_host_digest(host_name())}-"


def _host_digest(host: str) -> str:  # pragma: needs posix-hard-link
    # SHA-256 as host_name does: some runtimes, GraalPy's among them, have no BLAKE2 with a custom digest size.
    return hashlib.sha256(host.encode()).hexdigest()[:16]


def _remove_link_directory(link: _LinkDirectory) -> None:  # pragma: needs posix-hard-link
    if sys.platform == "win32":  # pragma: win32 cover
        _remove_link_directory_on_windows(link)
        return
    # A rename in the parent may have put any directory under this name, so entries go through a descriptor checked
    # to be the directory made, and the name goes only through rmdir, which refuses a non-empty directory.
    try:
        fd: Final = os.open(link.path, _DIRECTORY_FLAGS)
    except OSError:
        return
    try:
        opened: Final = os.fstat(fd)
        if (opened.st_dev, opened.st_ino) != link.identity:
            return
        for suffix in ("", "-journal", "-wal", "-shm"):
            with suppress(FileNotFoundError):
                os.unlink(f"{link.name}/{_LINK_NAME}{suffix}", dir_fd=fd)
        with suppress(FileNotFoundError):
            os.rmdir(link.name, dir_fd=fd)
    finally:
        os.close(fd)
    # The link is gone, so a failure here leaves one stray empty directory and is not worth failing a release for.
    with suppress(OSError):
        link.path.rmdir()


if sys.platform == "win32":  # pragma: win32 cover

    def _link_beside_on_windows(database: str, identity: _DatabaseIdentity) -> _LinkDirectory:
        parent: Final = pathlib.Path(database).parent
        # No uid exists here: a directory is a dead holder's only if its ACL admits nobody but the trusted accounts.
        _sweep_links_of_the_dead(parent, identity, 0)
        path: Final = parent / f"{_link_prefix()}{secrets.token_hex(8)}"
        _make_private_directory(os.fspath(path))
        link: _LinkDirectory | None = None
        try:
            # The handle keeps a rename in the parent from swapping the directory while the link is made inside it.
            with _pinned(os.fspath(path)) as directory_identity:
                link = _LinkDirectory(path, directory_identity, secrets.token_hex(8))
                _fill_link_directory(link, database, identity)
        except BaseException:
            if link is not None:  # pragma: no branch  # a pinned directory always gets its link
                _remove_link_directory(link)
            raise
        return link

    def _fill_link_directory(link: _LinkDirectory, database: str, identity: _DatabaseIdentity) -> None:
        if not _directory_is_private(os.fspath(link.path)):
            msg = f"link directory {os.fspath(link.path)!r} was replaced while opening"
            raise OSError(msg)
        _make_private_directory(os.fspath(link.path / link.name))
        os.link(database, link.database)
        # The open database handle pins its path against a rename, so only a file index that is not unique can differ.
        if _identity_at(os.fspath(link.database)) != identity:  # pragma: no cover
            msg = f"lock database changed while opening: {database!r}"
            raise OSError(msg)

    def _remove_dead_links_on_windows(path: pathlib.Path, identity: _DatabaseIdentity) -> None:
        with _pinned(os.fspath(path)):
            if not _directory_is_private(os.fspath(path)) or not _owner_is_gone(path.name):
                return
            with os.scandir(path) as entries:
                nested: Final = [pathlib.Path(entry.path) for entry in entries]
            for directory in nested:
                _unlink_link_to_on_windows(identity, directory / _LINK_NAME)
                with suppress(OSError):
                    directory.rmdir()
        with suppress(OSError):
            path.rmdir()

    def _unlink_link_to_on_windows(identity: _DatabaseIdentity, path: pathlib.Path) -> None:
        with suppress(OSError):
            if _identity_at(os.fspath(path)) == identity:
                path.unlink()

    def _remove_link_directory_on_windows(link: _LinkDirectory) -> None:
        # A removal never fails a release: SQLite or a scanner may still hold a name, and the next sweep retries.
        with suppress(OSError):
            with _pinned(os.fspath(link.path)) as directory_identity:
                if directory_identity != link.identity:
                    return
                for suffix in ("", "-journal", "-wal", "-shm"):
                    with suppress(FileNotFoundError):
                        pathlib.Path(f"{link.database}{suffix}").unlink()
                with suppress(FileNotFoundError):
                    pathlib.Path(link.database.parent).rmdir()
            link.path.rmdir()


def _open_lock_database(database: str, *, oplock_wait: float = 0) -> int:
    if sys.platform == "win32":  # pragma: win32 cover
        # The handle refuses a reparse point and omits delete sharing, so the name cannot be swapped while it is open.
        if (fd := _open_non_reparse_fd(database, 0o600, oplock_wait=oplock_wait)) is None:
            msg = f"lock database {database!r} is held without sharing or pending deletion"
            raise PermissionError(msg)
        return fd
    fd = os.open(database, _DB_OPEN_FLAGS, 0o600)  # pragma: win32 no cover
    if not stat.S_ISREG(os.fstat(fd).st_mode):  # pragma: win32 no cover
        os.close(fd)
        msg = f"refusing a non-regular lock database: {database!r}"
        raise OSError(msg)
    return fd  # pragma: win32 no cover


def _raise_if_wal(fd: int, database: str) -> None:
    # SQLite derives the -wal name from the path it connects through, a descriptor name on POSIX, and WAL never blocks
    # readers behind a writer, so the lock cannot work on such a database.
    # pread: an adopted probe is shared, so its file offset is not this connect's to move.
    header: Final = os.pread(fd, _HEADER_SIZE, 0) if hasattr(os, "pread") else os.read(fd, _HEADER_SIZE)
    if header.startswith(_SQLITE_MAGIC) and _WAL_FORMAT_VERSION in header[_FORMAT_VERSIONS]:
        msg = f"refusing a WAL-mode lock database: {database!r}; give ReadWriteLock a file of its own"
        raise ValueError(msg)


@contextmanager
def _bounded_schema(connection: sqlite3.Connection, database: str) -> Generator[None]:
    # The first statement loads the file's complete schema, which no timeout bounds, and a lock database has none.
    # Waiting for a lock runs no virtual machine steps, so a step budget bounds only that load.
    calls: Final = count()
    max_calls: Final = 10
    connection.setlimit(sqlite3.SQLITE_LIMIT_LENGTH, 4096)
    connection.set_progress_handler(lambda: next(calls) >= max_calls, 1000)
    try:
        yield
    except sqlite3.DatabaseError as error:
        # An interrupted schema load reports a malformed schema rather than the interrupt.
        if isinstance(error, sqlite3.DataError) or next(calls) > max_calls:
            msg = (
                f"refusing a lock database whose schema is too large to load: {database!r}; "
                "give ReadWriteLock a file of its own"
            )
            raise ValueError(msg) from error
        raise
    finally:
        connection.set_progress_handler(None, 0)


@contextmanager
def _sqlite_transition() -> Generator[None]:
    _SQLITE_TRANSITION_CONTEXT.depth += 1
    try:
        yield
    finally:
        _SQLITE_TRANSITION_CONTEXT.depth -= 1


def _abort_forked_sqlite_transition() -> None:  # pragma: forked child
    if _SQLITE_TRANSITION_CONTEXT.depth:
        os._exit(_UNSAFE_FORK_EXIT_STATUS)  # inherited SQLite handles cannot be used or closed safely


def _track_sqlite_use(event: str, _args: Unused) -> None:
    if event == "sqlite3.connect":
        _FORKED_DATABASES.note_sqlite_use()


def timeout_for_sqlite(timeout: float, *, blocking: bool, already_waited: float) -> int:
    if blocking is False:
        return 0

    if timeout == -1:
        return _MAX_SQLITE_TIMEOUT_MS

    if timeout < 0:
        msg = "timeout must be a non-negative number or -1"
        raise ValueError(msg)

    timeout_ms = int((max(timeout - already_waited, 0) if timeout > 0 else timeout) * 1000)
    if timeout_ms > _MAX_SQLITE_TIMEOUT_MS or timeout_ms < 0:
        _LOGGER.warning("timeout %s is too large for SQLite, using %s ms instead", timeout, _MAX_SQLITE_TIMEOUT_MS)
        return _MAX_SQLITE_TIMEOUT_MS
    return timeout_ms


_register_fork_object(_CONNECTION_ESCROW)
_register_fork_object(_FORKED_DATABASES)
_register_fork_object(_PROBE_DESCRIPTORS)
_register_fork_class(ReadWriteLock)
if _IS_PYPY:
    sys.addaudithook(_track_sqlite_use)  # pragma: pypy cover
if hasattr(os, "register_at_fork"):  # pragma: needs fork
    os.register_at_fork(after_in_child=_abort_forked_sqlite_transition)

__all__ = [
    "ReadWriteLock",
    "timeout_for_sqlite",
]
