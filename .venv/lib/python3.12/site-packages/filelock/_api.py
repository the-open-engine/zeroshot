from __future__ import annotations

__lazy_modules__ = [
    "atexit",
    "decimal",
    "filelock._error",
    "filelock._util",
    "inspect",
    "math",
    "numbers",
    "secrets",
    "types",
    "warnings",
]

import atexit
import contextlib
import inspect
import logging
import math
import os
import secrets
import stat
import sys
import time
import types
import warnings
from abc import ABCMeta, abstractmethod
from collections.abc import Callable, Hashable
from contextlib import contextmanager
from dataclasses import dataclass, fields
from decimal import Decimal
from functools import cache, partial
from itertools import count, starmap
from numbers import Real
from threading import TIMEOUT_MAX, Condition, RLock, Thread, current_thread, get_ident, local
from typing import TYPE_CHECKING, Final, Generic, Literal, NoReturn, TypedDict, TypeVar, cast
from weakref import WeakKeyDictionary, WeakValueDictionary, ref

from filelock._error import SoftFileLockLifetimeWarning, Timeout
from filelock._util import ensure_directory_exists

#: No explicit file permission mode was passed. Lock files then open with 0o666 so umask and default ACLs pick
#: the final permissions, and fchmod is skipped to preserve POSIX default ACL inheritance.
_UNSET_FILE_MODE: Final[int] = -1

_PERMISSION_BITS: Final[int] = stat.S_IRWXU | stat.S_IRWXG | stat.S_IRWXO

#: Ceiling on the retry counter used as a power of two, so a long contended wait cannot overflow the backoff multiply.
_MAX_BACKOFF_EXPONENT: Final[int] = 20

#: How a context manager reconciles a body failure with a release failure on exit (see the property of this name).
ContextErrorPolicy = Literal["chain", "group"]
_CONTEXT_ERROR_POLICIES: Final[frozenset[str]] = frozenset({"chain", "group"})

#: What a descriptor-owning backend does with an ``os.close`` failure after relinquishing ownership (see the property).
CloseErrorPolicy = Literal["default", "raise", "suppress"]
_CLOSE_ERROR_POLICIES: Final[frozenset[str]] = frozenset({"default", "raise", "suppress"})

if TYPE_CHECKING:
    from collections.abc import Generator, Mapping
    from threading import Lock
    from types import FrameType, TracebackType
    from typing import Protocol

    from _typeshed import Unused

    from filelock._lease import _LeaseClaim
    from filelock._read_write import ReadWriteLock
    from filelock._soft_rw import SoftReadWriteLock

    class _ForkResettable(Protocol):
        def _reset_after_fork_in_child(self) -> None: ...

    class _ForkDescriptorOwner(Protocol):
        def _descriptors_for_fork(self) -> tuple[tuple[int, tuple[int, int] | None], ...]: ...

    # Matched against the class object itself rather than `type[...]` of it. A metaclass supplies this method to the
    # class while leaving instances without it, so a `type[_ForkResettableClass]` bound rejects `ReadWriteLock`.
    class _ForkResettableClass(Protocol):
        def _reset_class_after_fork(self) -> None: ...

    class _RegisterAtFork(Protocol):
        def __call__(
            self,
            *,
            before: Callable[[], None] | None = None,
            after_in_parent: Callable[[], None] | None = None,
            after_in_child: Callable[[], None] | None = None,
        ) -> None: ...

    from typing import Self

_LOGGER: Final[logging.Logger] = logging.getLogger("filelock")
_REGISTER_AT_FORK: Final[_RegisterAtFork | None] = cast("_RegisterAtFork | None", getattr(os, "register_at_fork", None))
_HAS_REGISTER_AT_FORK: Final[bool] = _REGISTER_AT_FORK is not None

_ExtraValue = TypeVar("_ExtraValue")
_MarkerValue = TypeVar("_MarkerValue")
_SubclassValue = TypeVar("_SubclassValue")
_LockInitValue = float | int | bool | str | None | Callable[[int], None]


class LockOptions(TypedDict, total=False):
    """Every option the metaclass forwards, so a subclass adding its own can still type what it passes through."""

    timeout: float
    mode: int
    thread_local: bool
    blocking: bool
    is_singleton: bool
    poll_interval: float
    lifetime: float | None
    context_error_policy: ContextErrorPolicy
    close_error_policy: CloseErrorPolicy
    fallback_to_soft: bool
    preserve_lock_file: bool
    on_acquired: Callable[[int], None] | None


def _is_exception_group(error: BaseException) -> bool:
    return isinstance(error, BaseExceptionGroup)


def _raise_grouped_errors(
    message: str,
    first_error: BaseException,
    second_error: BaseException,
    *additional_errors: BaseException,
    marker: tuple[str, _MarkerValue] | None = None,
) -> NoReturn:
    errors = (first_error, second_error, *additional_errors)
    _detach_grouped_contexts(errors)
    group: Final = BaseExceptionGroup(message, errors)
    if marker is not None:
        setattr(group, marker[0], marker[1])
    raise group from None


def _detach_grouped_contexts(errors: tuple[BaseException, ...]) -> None:
    seen: set[int] = set()
    pending = list(errors)
    while pending:
        error = pending.pop()
        if id(error) in seen:
            continue
        seen.add(id(error))
        if (context := error.__context__) is not None and (
            context is error
            or _same_exception_tree(error, context)
            or any(context is root or _contains_exception(root, context) for root in errors)
        ):
            error.__context__ = None
        elif context is not None:
            pending.append(context)
        if error.__cause__ is not None:
            pending.append(error.__cause__)
        if _is_exception_group(error):
            pending.extend(cast("_ExceptionGroupProtocol", error).exceptions)


def _same_exception_tree(first: BaseException, second: BaseException) -> bool:
    pending = [(first, second)]
    seen: set[tuple[int, int]] = set()
    while pending:
        first_error, second_error = pending.pop()
        if first_error is second_error:
            continue
        if (pair := (id(first_error), id(second_error))) in seen:
            continue
        seen.add(pair)
        if (
            type(first_error) is not type(second_error)
            or not _is_exception_group(first_error)
            or not _is_exception_group(second_error)
        ):
            return False
        first_group = cast("_ExceptionGroupProtocol", first_error)
        second_group = cast("_ExceptionGroupProtocol", second_error)
        if first_group.message != second_group.message or len(first_group.exceptions) != len(second_group.exceptions):
            return False
        pending.extend(zip(first_group.exceptions, second_group.exceptions, strict=True))
    return True


def _contains_exception(error: BaseException, target: BaseException | None) -> bool:
    if target is None or not _is_exception_group(error):
        return False
    pending = list(cast("_ExceptionGroupProtocol", error).exceptions)
    seen: set[int] = set()
    while pending:
        child = pending.pop()
        if child is target:
            return True
        if id(child) in seen:
            continue
        seen.add(id(child))
        if _is_exception_group(child):
            pending.extend(cast("_ExceptionGroupProtocol", child).exceptions)
    return False


def _append_exception_context(error: BaseException, context: BaseException) -> None:
    if _exception_graph_contains(error, context) or _exception_graph_contains(context, error):
        return
    if error.__context__ is None:
        error.__context__ = context
        return
    tail = error
    seen: set[int] = set()
    while id(tail) not in seen:
        seen.add(id(tail))
        if (next_error := tail.__cause__ if tail.__cause__ is not None else tail.__context__) is None:
            tail.__context__ = context
            return
        tail = next_error


def _exception_graph_contains(error: BaseException, target: BaseException) -> bool:
    pending = [error]
    seen: set[int] = set()
    while pending:
        current = pending.pop()
        if current is target:
            return True
        if id(current) in seen:  # pragma: no cover - arbitrary caller exceptions can contain cycles
            continue
        seen.add(id(current))
        if current.__cause__ is not None:
            pending.append(current.__cause__)
        if current.__context__ is not None:
            pending.append(current.__context__)
        if _is_exception_group(current):
            pending.extend(cast("_ExceptionGroupProtocol", current).exceptions)
    return False


def _grouped_errors(
    error: BaseException, message: str, marker: tuple[str, _MarkerValue]
) -> tuple[BaseException, ...] | None:
    if not _is_exception_group(error):
        return None
    group = cast("_ExceptionGroupProtocol", error)
    return group.exceptions if group.message == message and getattr(group, marker[0], None) is marker[1] else None


if TYPE_CHECKING:

    class _ExceptionGroupProtocol(Protocol):
        @property
        def message(self) -> str: ...

        @property
        def exceptions(self) -> tuple[BaseException, ...]: ...


def _raise_chained_errors(first_error: BaseException, second_error: BaseException | None = None) -> NoReturn:
    if second_error is None:
        first_context = first_error.__context__
        try:
            raise first_error  # ruff:ignore[raise-within-try]  # the handler restores caller-supplied context before propagation
        except BaseException:
            first_error.__context__ = first_context
            raise
    if (second_context := second_error.__context__) is not None and second_context is not first_error:
        _detach_exception_context(second_context, first_error)
        _append_exception_context(first_error, second_context)
    first_context = first_error.__context__
    try:
        raise first_error  # ruff:ignore[raise-within-try]  # the second raise needs this error as implicit context
    except BaseException:  # ruff:ignore[blind-except]  # first_error may be a control-flow exception
        first_error.__context__ = first_context
        try:
            raise second_error  # ruff:ignore[raise-within-try]  # the handler makes the chain interpreter-independent
        except BaseException:
            second_error.__context__ = first_error
            first_error.__context__ = first_context
            raise


def _detach_exception_context(error: BaseException, target: BaseException) -> None:
    pending = [error]
    seen: set[int] = set()
    while pending:
        current = pending.pop()
        if id(current) in seen:
            continue
        seen.add(id(current))
        if current.__context__ is target:
            current.__context__ = None
        elif current.__context__ is not None:
            pending.append(current.__context__)
        if current.__cause__ is not None:
            pending.append(current.__cause__)
        if _is_exception_group(current):
            pending.extend(cast("_ExceptionGroupProtocol", current).exceptions)


def _raise_body_and_release(body_error: BaseException, release_error: BaseException) -> NoReturn:
    # Group mode: surface the body failure and the release failure as sibling leaves instead of letting one hide in the
    # other's __context__. BaseExceptionGroup returns a plain ExceptionGroup when both leaves subclass Exception, so
    # ``except*`` and ``except Exception`` still catch them; a BaseException leaf (KeyboardInterrupt, CancelledError)
    # keeps the group outside ordinary handlers. ``from None`` stops the group itself gaining a redundant __context__.
    _raise_grouped_errors("lock body and release both failed", body_error, release_error)


def _raise_cleanup_errors(
    message: str,
    primary_error: BaseException,
    *cleanup_errors: BaseException | None,
) -> NoReturn:
    _raise_grouped_errors(
        message,
        primary_error,
        *(error for error in cleanup_errors if error is not None),
    )


# On Windows os.path.realpath calls CreateFileW with share_mode=0, which blocks concurrent DeleteFileW and causes
# livelocks under threaded contention with SoftFileLock. os.path.abspath is purely string-based and avoids this.
_resolve_dir: Final[Callable[[str], str]] = os.path.abspath if sys.platform == "win32" else os.path.realpath


def _canonical(path: str | os.PathLike[str]) -> str:
    """
    Return one stable key for *path*, collapsing equivalent spellings without following a final symlink.

    Relative, absolute, and ``./`` spellings of one lock file must map to a single singleton instance, deadlock-registry
    entry, and removal key. Resolving the whole path with ``realpath`` would follow a final symlink and alias a lock
    target the backend deliberately rejects, so the registry identity would differ from the backend's. Resolving only
    the parent directory and re-appending the literal final component collapses the equivalent spellings while keeping a
    final symlink a distinct key. On Windows the parent is resolved with ``abspath`` so junctions and reparse points are
    not followed either.
    """
    parent, name = os.path.split(os.fspath(path))
    return os.path.join(_resolve_dir(parent or os.curdir), name)  # ruff:ignore[os-path-join]  # string join matches abspath/realpath


class _ThreadLocalRegistry(local):
    def __init__(self) -> None:
        super().__init__()
        self.held: dict[Hashable, int] = {}


_registry: Final[_ThreadLocalRegistry] = _ThreadLocalRegistry()


_T = TypeVar("_T", bound="BaseFileLock")
_LockT = TypeVar("_LockT", bound="BaseFileLock | ReadWriteLock | SoftReadWriteLock")


class FileLockMeta(ABCMeta):
    _instances: WeakValueDictionary[str, BaseFileLock]
    _instances_lock: RLock
    _instances_under_construction: set[str]

    def __call__(  # ruff:ignore[too-many-arguments]  # forwards the public constructor's documented parameters
        cls: type[_T],
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        mode: int = _UNSET_FILE_MODE,
        thread_local: bool = True,  # ruff:ignore[boolean-type-hint-positional-argument, boolean-default-value-positional-argument]  # public API: positional bool kept for backwards compatibility
        *,
        blocking: bool = True,
        is_singleton: bool = False,
        poll_interval: float = 0.05,
        lifetime: float | None = None,
        context_error_policy: ContextErrorPolicy = "chain",
        close_error_policy: CloseErrorPolicy = "default",
        fallback_to_soft: bool = True,
        preserve_lock_file: bool = False,
        on_acquired: Callable[[int], None] | None = None,
        **kwargs: _ExtraValue,
    ) -> _T:
        _ensure_current_process()
        _resolve_lazy_imports_before_shutdown()
        lifetime = _resolve_lifetime(lifetime, cls, stacklevel=cls._constructor_lifetime_warning_stacklevel)
        poll_interval = _resolve_poll_interval(poll_interval)
        # Validate before building the instance: a raise inside __init__ would leave a half-constructed object whose
        # __del__ then trips over the missing context.
        _check_blocking(blocking)
        # nan is left to acquire, which knows whether the call blocks; a non-blocking call ignores the timeout.
        timeout = _seconds("timeout", timeout)
        _check_mode(mode, cls.__name__)
        context_error_policy = _resolve_context_error_policy(context_error_policy)
        close_error_policy = _resolve_close_error_policy(close_error_policy)
        preserve_lock_file = _resolve_preserve_lock_file(
            preserve=preserve_lock_file, supported=cls._preserve_lock_file_supported, cls_name=cls.__name__
        )
        on_acquired = _resolve_on_acquired(on_acquired, supported=cls._on_acquired_supported, cls_name=cls.__name__)
        # Checked before the singleton lookup so a cache hit rejects a misspelled option just like a fresh build does.
        params: Final = _supported_init_params(
            cls,
            {
                "timeout": timeout,
                "mode": mode,
                "thread_local": thread_local,
                "blocking": blocking,
                "is_singleton": is_singleton,
                "poll_interval": poll_interval,
                "lifetime": lifetime,
                "context_error_policy": context_error_policy,
                "close_error_policy": close_error_policy,
                "fallback_to_soft": fallback_to_soft,
                "preserve_lock_file": preserve_lock_file,
                "on_acquired": on_acquired,
                **kwargs,
            },
        )
        if not is_singleton:
            return super().__call__(lock_file, **params)

        # Look up, build and store under one lock. Without it two threads racing the first construction for a
        # path both miss the cache and each build their own instance, so callers relying on is_singleton for
        # reentrant locking across instances end up with two "singletons" and acquire()'s deadlock check then
        # rejects a legitimate reentrant acquire; the unguarded writes to the WeakValueDictionary are a data
        # race besides. ReadWriteLock and SoftReadWriteLock already guard their singleton caches this way.
        # Key the cache on the canonical form so equivalent spellings of one path share a singleton, and it matches the
        # deadlock-registry key acquire() uses.
        singleton_key = _canonical(lock_file)
        with cls._instances_lock:
            # A cached lock resolves its relative path at each acquisition and pins it for the hold. After a chdir, or
            # once a parent symlink points elsewhere, an unheld lock may name another file than this key; a held one
            # names the file it pinned.
            if (instance := cls._instances.get(singleton_key)) is None or _canonical(
                instance._lock_path  # ruff:ignore[private-member-access]  # the metaclass owns the instance it returns
            ) != singleton_key:
                if singleton_key in cls._instances_under_construction:  # pragma: needs fork
                    msg = f"Singleton lock construction is already active for {lock_file!s}"
                    raise RuntimeError(msg)
                construction_registry = cls._instances_under_construction
                construction_pid = os.getpid()
                construction_registry.add(singleton_key)
                try:
                    instance = super().__call__(lock_file, **params)
                finally:
                    construction_registry.discard(singleton_key)
                if os.getpid() != construction_pid:  # pragma: needs fork
                    msg = "Lock construction cannot continue after fork; construct a new lock in the child"
                    raise RuntimeError(msg)
                cls._instances[singleton_key] = instance
                instance._singleton_key = singleton_key  # ruff:ignore[private-member-access]  # the cache entry the finalizer looks up
                return instance

        non_matching_params = {
            name: (str(passed_param), str(set_param))
            for name, (passed_param, set_param) in {
                "thread_local": (thread_local, instance.is_thread_local()),
                # Compare the wait, not the value: nan never equals itself, and -1, -5 and inf all mean no limit.
                "timeout": (
                    _timeout_behavior(timeout, any_negative=True),
                    _timeout_behavior(instance.timeout, any_negative=True),
                ),
                "mode": (mode, instance._context.mode),  # ruff:ignore[private-member-access]  # compares against the managed instance's own context
                "blocking": (blocking, instance.blocking),
                "poll_interval": (poll_interval, instance.poll_interval),
                "lifetime": (lifetime, instance.lifetime),
                "context_error_policy": (context_error_policy, instance.context_error_policy),
                "close_error_policy": (close_error_policy, instance.close_error_policy),
                "fallback_to_soft": (fallback_to_soft, instance.fallback_to_soft),
                "preserve_lock_file": (preserve_lock_file, instance.preserve_lock_file),
            }.items()
            if passed_param != set_param
        }
        # The metaclass owns the instance it returns, so reading the subclass hook stays inside this module's contract.
        non_matching_params.update(instance._singleton_extra_mismatches(kwargs))  # ruff:ignore[private-member-access]
        # Callables compare by identity, not equality: two equal callables can close over different state, so a
        # singleton must reject a different hook object even if it compares equal. Keep it out of the scalar dict above.
        hook_mismatch = on_acquired is not instance.on_acquired
        if not non_matching_params and not hook_mismatch:
            return instance  # ty: ignore[invalid-return-type]  # https://github.com/astral-sh/ty/issues/3231

        msg = "Singleton lock instances cannot be initialized with differing arguments"
        msg += "\nNon-matching arguments: "
        for param_name, (passed_param, set_param) in non_matching_params.items():
            msg += f"\n\t{param_name} (existing lock has {set_param} but {passed_param} was passed)"
        if hook_mismatch:
            msg += f"\n\ton_acquired (existing lock has {instance.on_acquired} but {on_acquired} was passed)"
        raise ValueError(msg)


def _resolve_lifetime(lifetime: float | None, cls: type[BaseFileLock], *, stacklevel: int) -> float | None:
    """
    Validate ``lifetime`` and drop a value the backend cannot honor.

    ``lifetime`` is a deliberate age-based lease: a lock file older than ``lifetime`` is broken even while its holder is
    still alive. Existence locks (:class:`SoftFileLock`) implement that behavior by unlinking a reclaimable pathname,
    which can overlap a live holder. A native OS lock lives on the inode, so unlinking the pathname by age cannot revoke
    the kernel lock; a contender would lock a fresh inode and overlap the live holder (#590). Ignore the request with a
    warning rather than accept a setting that breaks mutual exclusion.
    """
    if lifetime is None:
        return None
    # An int stays exact: no float holds every int 4.0.12 accepted here.
    if isinstance(lifetime, bool) or not isinstance(lifetime, int):
        lifetime = _seconds("lifetime", lifetime)
    if lifetime < 0 or (isinstance(lifetime, float) and not math.isfinite(lifetime)):
        msg = f"lifetime must be finite and non-negative, not {lifetime!r}"
        raise ValueError(msg)
    if not cls._lifetime_supported:
        warnings.warn(
            f"lifetime is ignored for {cls.__name__}: {cls._lifetime_unsupported_reason}; "
            f"only SoftFileLock supports lifetime-based expiry",
            stacklevel=stacklevel,
        )
        return None
    if cls._lifetime_replacements is not None:
        strict_lock, lease = cls._lifetime_replacements
        warnings.warn(
            f"{cls.__name__}(lifetime=...) uses age-based expiry and can overlap a live holder; "
            f"use {lease} for expiry or {strict_lock} for fail-closed locking",
            SoftFileLockLifetimeWarning,
            stacklevel=stacklevel,
        )
    return lifetime


def _resolve_poll_interval(poll_interval: float) -> float:
    """Validate ``poll_interval`` used for ``time.sleep`` / ``asyncio.sleep`` retries."""
    if (poll_interval := _seconds("poll_interval", poll_interval)) < 0 or not math.isfinite(poll_interval):
        msg = f"poll_interval must be finite and non-negative, not {poll_interval!r}"
        raise ValueError(msg)
    _check_timeout_max("poll_interval", poll_interval)
    return poll_interval


def _check_blocking(blocking: bool) -> None:  # ruff:ignore[boolean-type-hint-positional-argument]  # validates the option's own value
    if not isinstance(blocking, bool):
        msg = f"blocking must be a bool, not {type(blocking).__name__}"
        raise TypeError(msg)


def _seconds(name: str, value: float) -> float:
    # 4.0.12 accepted Decimal and Fraction; a bool is a typo, and a string would only fail later.
    if isinstance(value, bool) or not isinstance(value, (Real, Decimal)):
        msg = f"{name} must be a number of seconds, not {type(value).__name__}"
        raise TypeError(msg)
    # float() refuses a signaling NaN, which means no number of seconds just as a quiet one does.
    if isinstance(value, Decimal) and value.is_snan():
        return math.nan
    try:
        seconds: Final = float(value)
    except OverflowError:
        msg = f"{name} is too large to be a number of seconds: {value!r}"
        raise ValueError(msg) from None
    # float() rounds a tiny negative to -0.0, which compares as zero, yet timeout and lifetime checks read the sign.
    return -math.ulp(0.0) if seconds == 0 and value < 0 else seconds


def _check_mode(mode: int, cls_name: str) -> None:
    if mode == _UNSET_FILE_MODE:
        return
    if isinstance(mode, bool) or not isinstance(mode, int):
        msg = f"mode must be an int, not {type(mode).__name__}"
        raise TypeError(msg)
    # A lock file is never executed, so setuid, setgid and sticky have no meaning on it and only widen its permissions.
    if not 0 <= mode <= _PERMISSION_BITS:
        msg = f"mode={mode:#o} must be within 0o000-0o777 for {cls_name}"
        raise ValueError(msg)
    # A lock reopens, reads, or deletes the files it creates, so a mode that denies the owner read or write fails later
    # and for good.
    if ~mode & (stat.S_IRUSR | stat.S_IWUSR):
        msg = f"mode={mode:#o} must grant the owner read and write for {cls_name}"
        raise ValueError(msg)


def _resolve_context_error_policy(policy: str) -> ContextErrorPolicy:
    if policy not in _CONTEXT_ERROR_POLICIES:
        msg = f"context_error_policy must be 'chain' or 'group', got {policy!r}"
        raise ValueError(msg)
    return cast("ContextErrorPolicy", policy)


def _resolve_close_error_policy(policy: str) -> CloseErrorPolicy:
    if policy not in _CLOSE_ERROR_POLICIES:
        msg = f"close_error_policy must be 'default', 'raise', or 'suppress', got {policy!r}"
        raise ValueError(msg)
    return cast("CloseErrorPolicy", policy)


def _resolve_preserve_lock_file(*, preserve: bool, supported: bool, cls_name: str) -> bool:
    # An existence lock unlinks its marker to release, so preserving the pathname would defeat unlocking. Reject the
    # request rather than silently ignore it, since a caller asking for a stable identity must know it cannot be kept.
    if preserve and not supported:
        msg = f"preserve_lock_file=True is not supported by {cls_name}: unlinking its marker is how it releases"
        raise ValueError(msg)
    return preserve


def _resolve_on_acquired(
    on_acquired: Callable[[int], None] | None, *, supported: bool, cls_name: str
) -> Callable[[int], None] | None:
    if on_acquired is None:
        return None
    # An existence lock stores protocol state in its marker, so a caller writing through the descriptor would corrupt
    # stale detection and ownership metadata; only native locks lend out the descriptor.
    if not supported:
        msg = f"on_acquired is not supported by {cls_name}: only native locks expose the lock descriptor"
        raise ValueError(msg)
    return on_acquired


def _supported_init_params(
    cls: type[BaseFileLock], params: dict[str, _LockInitValue | _ExtraValue]
) -> dict[str, _LockInitValue | _ExtraValue]:
    if (model := _init_parameter_model(cls)).accepts_kwargs:
        return params
    if unsupported := sorted(
        name
        for name, value in params.items()
        if name not in model.accepted_params
        and ((parameter := model.default_params.get(name)) is None or value != parameter.default)
    ):
        msg = f"{cls.__name__} does not support non-default lock options: {', '.join(unsupported)}"
        raise TypeError(msg)
    # virtualenv narrows a BaseFileLock descendant's signature; omit base defaults it does not accept (#340).
    return {name: value for name, value in params.items() if name in model.accepted_params}


def _timeout_behavior(timeout: float, *, any_negative: bool) -> float | str:
    # inf, and for a BaseFileLock any negative value, wait without a limit like -1.
    if math.isnan(timeout):
        return "nan"
    return -1.0 if timeout == math.inf or (any_negative and timeout < 0) else timeout


def _check_timeout_max(name: str, value: float) -> None:
    # Lock.acquire, Event.wait and time.sleep raise OverflowError past this bound, which is only ~49.7 days on Windows.
    if value > TIMEOUT_MAX:
        msg = f"{name} must not exceed threading.TIMEOUT_MAX ({TIMEOUT_MAX}), got {value!r}"
        raise ValueError(msg)


_INIT_PARAMETER_MODELS: Final[WeakKeyDictionary[type[BaseFileLock], _InitParameterModel]] = WeakKeyDictionary()


def _init_parameter_model(cls: type[BaseFileLock]) -> _InitParameterModel:
    # A strong cache would keep dynamically created subclasses alive for the process lifetime.
    with _fork_transition(), _FORK_STATE.parameter_models_lock:
        if (model := _INIT_PARAMETER_MODELS.get(cls)) is None:
            parameters = inspect.signature(cls.__init__).parameters.values()
            model = _InitParameterModel(
                accepted_params=frozenset(
                    parameter.name
                    for parameter in parameters
                    if parameter.kind in {inspect.Parameter.POSITIONAL_OR_KEYWORD, inspect.Parameter.KEYWORD_ONLY}
                ),
                accepts_kwargs=any(parameter.kind is inspect.Parameter.VAR_KEYWORD for parameter in parameters),
                default_params={
                    name: parameter
                    for name, parameter in inspect.signature(type(cls).__call__).parameters.items()
                    if parameter.default is not inspect.Parameter.empty
                },
            )
            _INIT_PARAMETER_MODELS[cls] = model
        return model


@dataclass(frozen=True)
class _InitParameterModel:
    accepted_params: frozenset[str]
    accepts_kwargs: bool
    default_params: dict[str, inspect.Parameter]


class BaseFileLock(contextlib.ContextDecorator, metaclass=FileLockMeta):  # ruff:ignore[too-many-public-methods]  # public config properties
    """
    Abstract base class for a file lock object.

    Provides the common reentrant API and state management. Subclasses implement the locking mechanism
    (:class:`UnixFileLock <filelock.UnixFileLock>`, :class:`WindowsFileLock <filelock.WindowsFileLock>`,
    :class:`SoftFileLock <filelock.SoftFileLock>`).

    """

    _instances: WeakValueDictionary[str, BaseFileLock]
    _instances_lock: RLock
    _instances_under_construction: set[str]

    #: How the cross-instance deadlock message names the conflicting holder; the async subclass says "task".
    _deadlock_holder_desc: str = "FileLock instance in this thread"

    #: Whether an age-based :attr:`lifetime` lease may break this lock. Only existence locks set it (they reclaim by
    #: unlinking a pathname); native OS locks leave it ``False`` since a kernel lock cannot be revoked by file age.
    _lifetime_supported: bool = False

    #: Strict-lock and lease replacements for a backend with legacy age-based expiry.
    _lifetime_replacements: tuple[str, str] | None = None

    #: Why a backend that refuses ``lifetime`` cannot honor it, named in the warning that drops the value.
    _lifetime_unsupported_reason: str = "a native OS lock cannot be broken safely by file age"

    #: Async construction adds one metaclass frame before lifetime validation.
    _constructor_lifetime_warning_stacklevel: int = 3

    #: Whether :attr:`preserve_lock_file` may be ``True``. Native locks keep the pathname on release, so they support
    #: it; existence locks unlink their marker to release and reject it.
    _preserve_lock_file_supported: bool = True

    #: Whether an :attr:`on_acquired` hook may be set. Native locks lend the descriptor out; existence locks keep
    #: protocol state in the marker and reject it.
    _on_acquired_supported: bool = True

    #: Ceiling in seconds on the jittered backoff between contended acquisition retries. ``0`` keeps the fixed
    #: poll cadence; a multi-file backend sets it so contending processes desynchronize instead of livelocking.
    _poll_backoff_cap: float = 0.0

    #: The singleton cache key of this instance, or ``None`` outside the cache.
    _singleton_key: str | None = None

    def __init_subclass__(cls, **kwargs: _SubclassValue) -> None:
        """Give each lock subclass its own singleton registry and lock."""
        super().__init_subclass__(**kwargs)
        cls._instances = WeakValueDictionary()
        cls._instances_lock = RLock()
        cls._instances_under_construction = set()
        _register_fork_class(cls)

    @classmethod
    def _reset_class_after_fork(cls) -> None:  # pragma: forked child
        cls._instances = WeakValueDictionary()
        cls._instances_lock = RLock()
        cls._instances_under_construction = set()

    def __init__(  # ruff:ignore[too-many-arguments]  # public constructor: one parameter per documented lock option
        self,
        lock_file: str | os.PathLike[str],
        timeout: float = -1,
        mode: int = _UNSET_FILE_MODE,
        thread_local: bool = True,  # ruff:ignore[boolean-type-hint-positional-argument, boolean-default-value-positional-argument]  # public API: positional bool kept for backwards compatibility
        *,
        blocking: bool = True,
        is_singleton: bool = False,
        poll_interval: float = 0.05,
        lifetime: float | None = None,
        context_error_policy: ContextErrorPolicy = "chain",
        close_error_policy: CloseErrorPolicy = "default",
        fallback_to_soft: bool = True,
        preserve_lock_file: bool = False,
        on_acquired: Callable[[int], None] | None = None,
    ) -> None:
        """
        Create a new lock object.

        :param lock_file: path to the file
        :param timeout: default timeout when acquiring the lock, in seconds. It will be used as fallback value in the
            acquire method, if no timeout value (``None``) is given. If you want to disable the timeout, set it to a
            negative value or ``inf``. A timeout of 0 means that there is exactly one attempt to acquire the file lock.
        :param mode: file permissions for the lockfile. When not specified, the OS controls permissions via umask and
            default ACLs, preserving POSIX default ACL inheritance in shared directories. Construction raises
            :class:`ValueError` unless it grants the owner read and write, which the lock needs for its own files. On
            Windows a native lock creates its file with an owner-only ACL when the mode has no group and other bits; any
            other mode, and a soft lock, leave the file the ACL its directory hands down. A Unix lock file that already
            exists keeps its permissions.
        :param thread_local: Whether this object's internal context should be thread local or not. If this is set to
            ``False`` then the lock will be reentrant across threads. When ``True`` (the default), **all fields of the
            lock's internal context are per-thread**, including the configuration values ``poll_interval``, ``timeout``,
            ``blocking``, ``mode``, and ``lifetime``. Setting one of these properties from one thread does not change
            the value seen by another thread; threads that did not perform the write continue to see the value supplied
            at construction time. ``mode`` has no setter, so construction is the only place it is ever set. If you need
            configuration values to be visible across threads, construct the lock with ``thread_local=False``.
        :param blocking: whether the lock should be blocking or not
        :param is_singleton: If this is set to ``True`` then only one instance of this class will be created per lock
            file. This is useful if you want to use the lock object for reentrant locking without needing to pass the
            same object around.
        :param poll_interval: default interval for polling the lock file, in seconds. It will be used as fallback value
            in the acquire method, if no poll_interval value (``None``) is given.
        :param lifetime: for :class:`SoftFileLock`, the age in seconds after which a waiting process may delete the
            marker, even while its holder remains alive. This legacy expiry mode does not provide strict mutual
            exclusion. ``None`` (the default) disables age-based expiry. Native OS locks (:class:`FileLock`) cannot be
            revoked by file age and ignore a non-``None`` ``lifetime`` with a warning.
        :param context_error_policy: how a context manager reconciles a failure in its body with a failure while
            releasing on exit. ``"chain"`` (the default) keeps Python's behavior: the release error propagates with the
            body error in its ``__context__``. ``"group"`` raises a :class:`BaseExceptionGroup` holding the body error
            first and the release error second, so neither hides the other.
        :param close_error_policy: what to do with an ``os.close`` failure after relinquishing descriptor ownership.
            ``"default"`` keeps each backend's historical behavior (Unix native locks drop a FUSE/Docker ``EIO``;
            Windows native locks and :class:`SoftFileLock` propagate); ``"raise"`` always propagates the ``OSError``;
            ``"suppress"`` always ignores it. Held state is released either way. It does not affect unlock failures or
            lock-file deletion.
        :param fallback_to_soft: for :class:`UnixFileLock`, whether to switch to :class:`SoftFileLock` when the
            filesystem's ``flock`` returns ``ENOSYS``. ``True`` (the default) keeps the historical fallback;
            ``False`` fails closed, letting the ``ENOSYS`` propagate so a caller that needs kernel-enforced
            locking is never silently downgraded. It has no effect on Windows or :class:`SoftFileLock`.
        :param preserve_lock_file: for native locks (:class:`FileLock`), whether filelock promises not to unlink the
            lock pathname on release. ``False`` (the default) keeps each backend's cleanup: Windows removes the lock
            file, Unix already leaves it. ``True`` keeps a stable file identity for ACLs, auditing, and holder metadata:
            Windows skips its post-release unlink and Unix refuses to enter the ``ENOSYS`` soft fallback (which releases
            by unlinking). :class:`SoftFileLock` rejects ``True``. The promise covers filelock's own release path only;
            it cannot stop another process or the filesystem from removing the pathname.
        :param on_acquired: for native locks (:class:`FileLock`), a callable invoked with the borrowed lock descriptor
            once per physical acquisition, after filelock holds the native lock and finished backend initialization but
            before :meth:`~BaseFileLock.acquire` returns. Recursive acquisitions do not call it again. The callback may
            read, write, seek, truncate, or set metadata through ``os`` on the descriptor, but must not close, unlock,
            or take ownership of it, and filelock does not fsync its writes. If it raises, filelock releases the lock
            and re-raises. :class:`SoftFileLock` rejects the hook.

        :raises TypeError: if *timeout* or *poll_interval* is a :class:`bool` or not a real number, *blocking* is not a
            :class:`bool`, or *mode* is not an :class:`int`
        :raises ValueError: if *poll_interval* is negative, not finite, or above :data:`threading.TIMEOUT_MAX`, if
            *timeout* is too large to convert to :class:`float`, or if *mode* falls outside ``0o000``-``0o777`` or
            denies the owner read and write

        """
        self._creator_pid = os.getpid()
        #: Holder id in the deadlock registry; a stand-in that finishes the finalizer has another id(self).
        self._holder_id = id(self)
        self._transition_lock = RLock()
        self._is_thread_local = thread_local
        self._is_singleton = is_singleton
        self._context_error_policy = context_error_policy  # already validated by the metaclass
        self._close_error_policy = close_error_policy  # already validated by the metaclass
        self._fallback_to_soft = fallback_to_soft
        self._preserve_lock_file = preserve_lock_file  # already validated by the metaclass
        self._on_acquired = on_acquired  # already validated by the metaclass

        self._context: FileLockContext = (ThreadLocalFileContext if thread_local else FileLockContext)(
            lock_file=os.fspath(lock_file),
            timeout=timeout,
            mode=mode,
            blocking=blocking,
            poll_interval=poll_interval,
            lifetime=lifetime,
        )
        #: Each thread's committed hold on a thread-local lock, so another thread can release it.
        self._thread_holds: dict[int, tuple[FileLockContext, Thread]] = {}
        #: Each thread's uncommitted acquisition, so a backend can finish it for an exited thread.
        self._thread_pending: dict[int, tuple[FileLockContext, Thread]] = {}
        _register_fork_object(self)

    def _singleton_extra_mismatches(  # ruff:ignore[no-self-use]  # the base class adds no options of its own
        self, _kwargs: Mapping[str, _ExtraValue], /
    ) -> dict[str, tuple[str, str]]:
        # A subclass's own options reach the metaclass only as kwargs, and only the subclass knows their defaults and
        # how __init__ resolves them, so each override compares its options and merges what super() reports.
        return {}

    def is_thread_local(self) -> bool:
        """:returns: a flag indicating if this lock is thread local or not"""
        return self._is_thread_local

    @property
    def is_singleton(self) -> bool:
        """
        A flag indicating if this lock is singleton or not.

        .. versionadded:: 3.13.0

        """
        return self._is_singleton

    @property
    def context_error_policy(self) -> ContextErrorPolicy:
        """
        How a context manager reconciles a body failure with a release failure on exit.

        .. versionadded:: 3.30.0

        """
        return self._context_error_policy

    @property
    def close_error_policy(self) -> CloseErrorPolicy:
        """
        What a lock does with an ``os.close`` failure after relinquishing descriptor ownership.

        .. versionadded:: 3.30.0

        """
        return self._close_error_policy

    def _close_released_fd(self, fd: int, *, default_suppresses: bool) -> None:
        # CPython never retries close() after EINTR because the descriptor number may already be reused, so neither does
        # this. close_error_policy decides the error's fate after the backend relinquishes descriptor ownership.
        try:
            os.close(fd)
        except OSError:
            if not self._suppresses_close_error(default_suppresses=default_suppresses):
                raise

    def _suppresses_close_error(self, *, default_suppresses: bool) -> bool:
        return self._close_error_policy == "suppress" or (self._close_error_policy == "default" and default_suppresses)

    @property
    def fallback_to_soft(self) -> bool:
        """
        Whether a :class:`FileLock` falls back to :class:`SoftFileLock` when the filesystem lacks ``flock``.

        Only :class:`UnixFileLock` acts on it: when ``False`` an ``ENOSYS`` from ``flock`` propagates instead of
        switching to existence-lock semantics.

        .. versionadded:: 3.30.0

        """
        return self._fallback_to_soft

    @property
    def preserve_lock_file(self) -> bool:
        """
        Whether filelock promises not to unlink the lock pathname on release.

        When ``True``, Windows skips its post-release unlink and Unix refuses the ``ENOSYS`` soft fallback.
        :class:`SoftFileLock` rejects ``True`` because unlinking its marker is how it releases.

        .. versionadded:: 3.30.0

        """
        return self._preserve_lock_file

    @property
    def on_acquired(self) -> Callable[[int], None] | None:
        """
        The callback run with the borrowed lock descriptor once per physical acquisition, or ``None``.

        Native locks only. It runs after the native lock is held and backend initialization finished, before
        :meth:`~BaseFileLock.acquire` returns; a raise rolls back the acquisition. :class:`SoftFileLock` rejects it.

        .. versionadded:: 3.30.0

        """
        return self._on_acquired

    @property
    def lock_file(self) -> str:
        """Path to the lock file."""
        return self._context.lock_file

    @property
    def _lock_path(self) -> str:
        # The OS resolves a relative lock_file against the working directory at each call. A chdir during an
        # acquisition or a hold would otherwise make the backend wait on, lock or remove a different file.
        return self._context.claim_root or self._context.lock_file

    @property
    def timeout(self) -> float:
        """
        The default timeout value, in seconds.

        .. versionadded:: 2.0.0

        """
        return self._context.timeout

    @timeout.setter
    def timeout(self, value: float | str | bytes | bytearray) -> None:
        """
        Change the default timeout value.

        :param value: the new value, in seconds; a negative value or ``inf`` waits without a limit

        :raises TypeError: if *value* is a :class:`bool` or not a real number
        :raises ValueError: if *value* is too large to convert to :class:`float`

        .. deprecated:: 4.0.13

            Passing a numeric string; pass the number instead.

        """
        if isinstance(value, (str, bytes, bytearray)):
            warnings.warn(
                "setting timeout from a string is deprecated and will be removed; pass a number",
                DeprecationWarning,
                stacklevel=2,
            )
            value = float(value)
        self._context.timeout = _seconds("timeout", value)

    @property
    def blocking(self) -> bool:
        """
        Whether the locking is blocking or not.

        .. versionadded:: 3.14.0

        """
        return self._context.blocking

    @blocking.setter
    def blocking(self, value: bool) -> None:
        """
        Change the default blocking value.

        :param value: the new value as bool

        :raises TypeError: if *value* is not a bool

        """
        _check_blocking(value)
        self._context.blocking = value

    @property
    def poll_interval(self) -> float:
        """
        The default polling interval, in seconds.

        .. versionadded:: 3.24.0

        """
        return self._context.poll_interval

    @poll_interval.setter
    def poll_interval(self, value: float) -> None:
        """
        Change the default polling interval.

        :param value: the new value, in seconds

        :raises ValueError: if *value* is negative, not finite, or above :data:`threading.TIMEOUT_MAX`
        :raises TypeError: if *value* is a :class:`bool` or not a real number

        """
        self._context.poll_interval = _resolve_poll_interval(value)

    @property
    def lifetime(self) -> float | None:
        """
        The soft marker age in seconds that permits expiry, or ``None`` to disable age-based expiry.

        A non-``None`` value permits a waiter to enter while the previous holder remains active, so it does not provide
        strict mutual exclusion. Native locks ignore the value with a warning.

        .. versionadded:: 3.24.0

        """
        return self._context.lifetime

    @lifetime.setter
    def lifetime(self, value: float | None) -> None:
        """
        Change the legacy age-based expiry threshold.

        :param value: the new value in seconds, or ``None`` to disable expiration

        :raises ValueError: if *value* is negative or not finite
        :raises TypeError: if *value* is not ``None`` and not a real number

        """
        self._context.lifetime = _resolve_lifetime(value, type(self), stacklevel=3)

    @property
    def mode(self) -> int:
        """
        The file permissions requested for the lock file.

        Without an explicit ``mode`` this reports ``0o644`` for backwards compatibility, but the file is created with
        ``0o666`` filtered through the umask and default ACLs, so with a ``002`` umask it ends up ``0o664``. Check
        :attr:`has_explicit_mode` before relying on the value, or ``os.stat`` the file for its actual permissions.

        """
        return 0o644 if self._context.mode == _UNSET_FILE_MODE else self._context.mode

    @property
    def has_explicit_mode(self) -> bool:
        """Whether the file permissions were explicitly set."""
        return self._context.mode != _UNSET_FILE_MODE

    def _open_mode(self) -> int:
        """Mode for ``os.open``: 0o666 when unset so umask and ACLs decide, otherwise the explicit mode."""
        return 0o666 if self._context.mode == _UNSET_FILE_MODE else self._context.mode

    @property
    def is_locked(self) -> bool:
        """
        A boolean indicating if the lock file is holding the lock currently.

        .. versionchanged:: 2.0.0

            This was previously a method and is now a property.

        """
        _ensure_current_process()
        return self._context.lock_file_fd is not None

    @property
    def lock_counter(self) -> int:
        """The number of times this lock has been acquired (but not yet released)."""
        _ensure_current_process()
        return self._context.lock_counter

    def __enter__(self) -> Self:
        """
        Acquire the lock.

        :returns: the lock object

        """
        self.acquire()
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        """Release the lock, reconciling a release failure with any body failure per :attr:`context_error_policy`."""
        self._release_in_context(exc_value)

    def _release_in_context(self, body_error: BaseException | None) -> None:
        # Release from a context-manager exit. "chain" lets a release failure propagate with the body error already in
        # its __context__ (Python's default); "group" raises both as sibling leaves so neither one hides the other.
        try:
            self.release()
        except BaseException as release_error:
            if body_error is None or self._context_error_policy == "chain":
                raise
            _raise_body_and_release(body_error, release_error)

    def __del__(self) -> None:
        """Force-release so a dropped reference never leaks a held lock, including every thread's hold."""
        if vars(self).get("_creator_pid") != os.getpid():
            return  # pragma: forked child
        # CPython runs __del__ before it clears weak references. Until then the singleton cache still hands this lock
        # out, and a thread that takes a hold through it would lose that hold to a release here. The cycle collector
        # clears the references first, and at exit no other thread runs.
        if (key := self._singleton_key) is not None and not sys.is_finalizing() and self._instances.get(key) is self:
            self._release_once_unreachable()
        else:
            self._release_abandoned_holds()

    def _release_abandoned_holds(self) -> None:
        # A finalizer must not raise. A release error during garbage collection would otherwise surface as an
        # unraisable-exception warning, attributed to whichever code triggered collection. The dropped lock still gets
        # best-effort cleanup; an explicit release() reports the same error to a caller who can act on it.
        with contextlib.suppress(Exception):
            self.release(force=True)
        # Nothing references the lock any more, so no thread can still be inside it through this instance.
        with contextlib.suppress(Exception):
            self._release_thread_holds(exited_only=False)

    def _release_once_unreachable(self) -> None:
        # The callback runs once the lock is gone, so it reads the state through a holder. The state alone keeps the
        # holder alive: a reference from the callback would keep the state, and through it the lock, alive.
        self._finalizer_state = _FinalizerState(vars(self))
        _UNREACHABLE_LOCKS.add(ref(self, partial(_finalize_unreachable, ref(self._finalizer_state), type(self))))

    def acquire(
        self,
        timeout: float | None = None,
        poll_interval: float | None = None,
        *,
        poll_intervall: float | None = None,
        blocking: bool | None = None,
        cancel_check: Callable[[], bool] | None = None,
    ) -> AcquireReturnProxy[Self]:
        """
        Try to acquire the file lock.

        :param timeout: maximum wait time for acquiring the lock, ``None`` means use the default :attr:`~timeout` is and
            if ``timeout < 0``, there is no timeout and this method will block until the lock could be acquired
        :param poll_interval: interval of trying to acquire the lock file, ``None`` means use the default
            :attr:`~poll_interval`
        :param poll_intervall: deprecated, kept for backwards compatibility, use ``poll_interval`` instead
        :param blocking: whether to retry until this call holds the lock or the timeout expires, ``None`` means use
            the default :attr:`~blocking`. ``False`` makes one attempt and raises :class:`~Timeout` on failure.
        :param cancel_check: a callable returning ``True`` when the acquisition should be canceled. Checked on each poll
            iteration. When triggered, raises :class:`~Timeout` just like an expired timeout.

        :returns: a context object that will unlock the file when the context is exited

        :raises Timeout: if fails to acquire lock within the timeout period
        :raises TypeError: if *blocking* is not a :class:`bool`
        :raises ValueError: if a blocking call gets a ``nan`` timeout, or *poll_interval* exceeds
            :data:`threading.TIMEOUT_MAX`

        .. code-block:: python

            # You can use this method in the context manager (recommended)
            with lock.acquire():
                pass

            # Or use an equivalent try-finally construct:
            lock.acquire()
            try:
                pass
            finally:
                lock.release()

        .. versionchanged:: 2.0.0

            This method returns now a *proxy* object instead of *self*, so that it can be used in a with statement
            without side effects.

        """
        self._raise_if_inherited()
        if timeout is None:
            timeout = self._context.timeout

        if blocking is None:
            blocking = self._context.blocking

        # A truthy non-bool such as the string "false" would otherwise turn a single attempt into a blocking wait.
        _check_blocking(blocking)

        if poll_intervall is not None:
            msg = "use poll_interval instead of poll_intervall"
            warnings.warn(msg, DeprecationWarning, stacklevel=2)
            poll_interval = poll_intervall

        poll_interval = poll_interval if poll_interval is not None else self._context.poll_interval
        poll_interval = _resolve_poll_interval(poll_interval)
        timeout = _resolve_timeout(timeout, blocking=blocking)

        start_time = time.perf_counter()
        # Wait for admission before touching any state: a caller refused entry must leave the counter, the registry and
        # the descriptor exactly as it found them.
        with self._transition_admission(
            blocking=blocking,
            cancel_check=cancel_check,
            timeout=timeout,
            poll_interval=poll_interval,
            start_time=start_time,
        ):
            # The interrupted acquire cannot run until the handler returns, so a second one here would wait on it
            # forever or take the lock out from under it. Once the lock is held, as in an on_acquired hook, it nests.
            if not self.is_locked and _on_this_stack(self._context.acquiring):
                msg = f"Deadlock: lock '{self.lock_file}' is acquired again while this thread's acquire of it waits"
                raise RuntimeError(msg)
            # An interrupted release commits only the hold it started with and ignores a release above it on the
            # stack. A hold taken here would nest into one the OS dropped, or stay held for good.
            if _on_this_stack(self._context.releasing):
                msg = f"lock '{self.lock_file}' is acquired again while this thread's release of it runs"
                raise RuntimeError(msg)
            canonical = _canonical(self.lock_file)
            self._context.lock_counter += 1

            self._raise_if_would_deadlock(canonical, timeout=timeout, blocking=blocking)
            if not self.is_locked:
                self._pin_lock_path(canonical)

            try:
                self._poll_until_acquired(
                    blocking=blocking,
                    cancel_check=cancel_check,
                    timeout=timeout,
                    poll_interval=poll_interval,
                    start_time=start_time,
                )
            except BaseException:
                self._reconcile_failed_acquire(canonical)
                raise
            self._commit_acquire(canonical)
            return AcquireReturnProxy(lock=self)

    @contextlib.contextmanager
    def _transition_admission(
        self,
        *,
        blocking: bool,
        cancel_check: Callable[[], bool] | None,
        timeout: float,
        poll_interval: float,
        start_time: float,
    ) -> Generator[None]:
        # One thread at a time drives the physical transition of a shared instance: its counter, descriptor and claim
        # state are plain attributes every thread reads and writes, so an unserialized loser clears the winner's
        # descriptor or double-closes it. Waiting in slices keeps cancel_check and the timeout live. A thread-local
        # context is never shared, so it skips the gate.
        if self._is_thread_local:
            yield
            return
        deadline: Final = None if timeout < 0 else start_time + timeout
        wait: Final = poll_interval if blocking else 0.0
        # wait leads the min() so a non-blocking call, whose nan timeout goes unchecked, waits 0 rather than nan.
        while not self._transition_lock.acquire(
            timeout=wait if deadline is None else min(wait, max(deadline - time.perf_counter(), 0.0))
        ):
            if not blocking or (cancel_check is not None and cancel_check()):
                raise Timeout(self.lock_file)
            if deadline is not None and time.perf_counter() >= deadline:
                raise Timeout(self.lock_file)
        try:
            yield
        finally:
            self._transition_lock.release()

    def release(self, force: bool = False) -> None:  # ruff:ignore[boolean-type-hint-positional-argument, boolean-default-value-positional-argument]  # public API: positional bool kept for backwards compatibility
        """
        Release the file lock. The lock is only completely released when the lock counter reaches 0. The lock file
        itself may be deleted automatically, the behavior is platform-specific.

        :param force: If true, the lock counter is ignored and the lock is released in every case. On a thread-local
            lock it also releases the holds of threads that exited without releasing; a hold of a thread still running
            stays with that thread. Dropping the last reference to the lock releases every thread's hold.

        """
        counter_before: int | None = None
        try:
            counter_before = self._context.lock_counter
            self._release_current(force=force)
        except BaseException as release_error:
            # A handler that raised after release() began and before the backend changed anything leaves the counter as
            # it was; only then does a second attempt release the hold the caller gave up. A failed system call keeps it
            # for an explicit retry, and a changed counter means a nested level or the backend's own retry already ran.
            if (
                counter_before is not None
                and counter_before == self._context.lock_counter
                and self._context.lock_file_fd is not None
                and not _is_system_call_failure(release_error)
            ):
                with contextlib.suppress(Exception):
                    self._release_current(force=force)
            raise
        if force:
            self._release_thread_holds(exited_only=True)

    def _release_current(self, *, force: bool) -> None:
        # A shared instance releases under the same gate its acquisition ran through, so a thread entering the lock
        # never observes a partially torn-down owner. Only an acquirer that found the lock free holds the gate for
        # long, so an unheld lock returns without waiting behind it.
        if self._context.lock_file_fd is None:
            # Only a committed hold registers, so an entry without a descriptor is a release a signal handler stopped
            # after the unlock; left alone it refuses every other instance on this path in this thread.
            if self._context.lock_file_registry is not None:
                self._finish_interrupted_release()
            return
        with contextlib.nullcontext() if self._is_thread_local else self._transition_lock:
            state: Final = self._hold_state()
            # A signal handler can land between the backend's unlock and its clearing of the descriptor, where a second
            # release would close the number twice, maybe hitting a file that reused it. The flag holds the releasing
            # frame, so only a release still on this stack stops it, and a flag a cut-short frame left set does not.
            if self._creator_pid != os.getpid() or state.lock_file_fd is None or _on_this_stack(state.releasing):
                return
            if not force and state.lock_counter > 1:
                state.lock_counter -= 1
                return
            state.releasing = inspect.currentframe()
            try:
                _LOGGER.debug("Attempting to release lock %s on %s", id(self), self.lock_file)
                self._release_with_fork_tracking()
            finally:
                # A failure after the OS unlock (during close or unlink) still released the lock: the backend cleared
                # its descriptor, so commit the counter and registry to released even as the cleanup error propagates.
                # A failure that left the lock held keeps the counter so a later release can retry the OS unlock.
                if state.lock_file_fd is None:  # pragma: no branch  # a release that returns cleared the descriptor
                    self._commit_release()
                state.releasing = None
            _LOGGER.debug("Lock %s released on %s", id(self), self.lock_file)

    def _finish_interrupted_release(self) -> None:
        # An unheld lock never waits behind an acquirer, whose own commit takes the entry over anyway.
        if not self._transition_lock.acquire(blocking=False):
            return  # pragma: no cover  # only a concurrent acquire of a shared instance holds the gate
        try:
            if self._context.lock_file_fd is None:  # pragma: no branch  # only a racing acquire fills it
                self._commit_release()
        finally:
            self._transition_lock.release()

    def _hold_state(self) -> FileLockContext:
        return self._context.current if isinstance(self._context, ThreadLocalFileContext) else self._context

    def _release_thread_holds(self, *, exited_only: bool) -> None:
        if not isinstance(context := self._context, ThreadLocalFileContext):
            return
        # Serialized, or two threads would both close an exited thread's descriptor; copy() is atomic free-threaded.
        with self._transition_lock:
            own: Final = context.current
            for held, thread in self._thread_holds.copy().values():
                if exited_only and thread.is_alive():
                    continue
                context.current = held
                try:
                    self._release_current(force=True)
                finally:
                    context.current = own

    def _raise_if_inherited(self) -> None:
        if self._creator_pid != os.getpid():  # pragma: forked child
            msg = f"{type(self).__name__} on {self.lock_file} was inherited across fork; construct a new instance"
            raise RuntimeError(msg)

    def _mark_descriptor_owned(self, fd: int, identity: tuple[int, int] | None = None) -> None:
        state: Final = self._hold_state()
        # Held first, pending cleared second: an interrupt between the two then leaves the descriptor recorded.
        state.lock_file_fd = fd
        state.lock_file_fd_identity = identity
        state.pending_lock_file_fd = None
        state.pending_lock_file_fd_identity = None
        if isinstance(self._context, ThreadLocalFileContext):
            self._thread_pending.pop(id(state), None)
            self._thread_holds[id(state)] = state, current_thread()

    def _mark_descriptor_pending(self, fd: int, identity: tuple[int, int] | None = None) -> None:
        state: Final = self._hold_state()
        state.pending_lock_file_fd = fd
        state.pending_lock_file_fd_identity = identity
        if isinstance(self._context, ThreadLocalFileContext):
            self._thread_pending[id(state)] = state, current_thread()

    def _mark_descriptor_released(self) -> None:
        state: Final = self._hold_state()
        state.pending_lock_file_fd = None
        state.pending_lock_file_fd_identity = None
        state.lock_file_fd = None
        state.lock_file_fd_identity = None
        if isinstance(self._context, ThreadLocalFileContext):
            self._thread_pending.pop(id(state), None)
            self._thread_holds.pop(id(state), None)

    def _reset_after_fork_in_child(self) -> None:  # pragma: forked child
        # fork copies the lock in whatever state the parent's threads left it, so give the child an unheld one.
        self._transition_lock = RLock()
        if isinstance(self._context, ThreadLocalFileContext):
            self._thread_holds.clear()
            self._thread_pending.clear()
        self._context.owner_claim_paths = ()
        self._context.claim_root = None
        self._context.lock_file_fd = None
        self._context.lock_file_fd_token = None
        self._context.lock_file_fd_identity = None
        self._context.pending_lock_file_fd = None
        self._context.pending_lock_file_fd_identity = None
        self._context.lock_counter = 0
        self._context.releasing = None
        self._context.acquiring = None
        self._context.lock_file_key = None
        self._context.lock_file_registry = None
        self._context.lease_claim = None

    def _descriptors_for_fork(self) -> tuple[tuple[int, tuple[int, int] | None], ...]:  # pragma: needs fork
        descriptors: list[tuple[int, tuple[int, int] | None]] = []
        if self._context.lock_file_fd is not None and self._context.lock_file_fd_token is None:
            descriptors.append((self._context.lock_file_fd, self._context.lock_file_fd_identity))
        if self._context.pending_lock_file_fd is not None:
            descriptors.append((self._context.pending_lock_file_fd, self._context.pending_lock_file_fd_identity))
        return tuple(descriptors)

    def _raise_if_would_deadlock(self, canonical: str, *, timeout: float, blocking: bool) -> None:
        """
        Fail fast when a *different* live instance already holds this path in the current deadlock scope.

        Only the first, indefinitely-blocking acquire can self-deadlock this way: waiting in the OS primitive would
        block on a lock this flow already owns. A finite timeout or ``blocking=False`` keeps the normal Timeout path.
        """
        would_block = self._context.lock_counter == 1 and not self.is_locked and timeout < 0 and blocking
        if would_block and _registry.held.get(self._registry_key(canonical)) not in {None, self._holder_id}:
            self._context.lock_counter -= 1
            msg = (
                f"Deadlock: lock '{self.lock_file}' is already held by a different {self._deadlock_holder_desc}. "
                f"Use is_singleton=True to enable reentrant locking across instances."
            )
            raise RuntimeError(msg)

    def _registry_key(self, canonical: str) -> Hashable:
        return canonical if (scope := self._deadlock_scope()) is None else (scope, canonical)

    @staticmethod
    def _deadlock_scope() -> Hashable | None:
        """
        Execution unit whose own hold would deadlock a new acquire.

        ``None`` scopes holders to the thread, which the thread-local registry already separates. Async locks
        override it with the running task, since one event loop thread runs many tasks and only a reacquire from
        the *same* task can self-deadlock.
        """
        return None

    def _poll_until_acquired(
        self,
        *,
        blocking: bool,
        cancel_check: Callable[[], bool] | None,
        timeout: float,
        poll_interval: float,
        start_time: float,
    ) -> None:
        lock_id = id(self)
        lock_filename = self.lock_file
        attempt = 0
        self._context.acquiring = inspect.currentframe()
        try:
            # Created once, so a directory that disappears while the acquisition waits fails it rather than building a
            # second lock file beside the holder's, and not on re-entry, which joins a hold whose parent may be renamed.
            if not self.is_locked:
                ensure_directory_exists(self._lock_path)
            while True:
                self._raise_if_inherited()
                if not self.is_locked:
                    self._try_break_expired_lock()
                    _LOGGER.debug("Attempting to acquire lock %s on %s", lock_id, lock_filename)
                    self._acquire_with_fork_tracking()
                    self._raise_if_inherited()
                if self.is_locked:
                    _LOGGER.debug("Lock %s acquired on %s", lock_id, lock_filename)
                    return
                if self._check_give_up(
                    blocking=blocking,
                    cancel_check=cancel_check,
                    timeout=timeout,
                    start_time=start_time,
                ):
                    raise Timeout(lock_filename, self._unprobeable_holder())
                attempt += 1
                delay = self._poll_delay(poll_interval, attempt)
                if timeout >= 0:
                    delay = min(delay, max(start_time + timeout - time.perf_counter(), 0.0))
                msg = "Lock %s not acquired on %s, waiting %s seconds ..."
                _LOGGER.debug(msg, lock_id, lock_filename, delay)
                time.sleep(delay)
        finally:
            self._context.acquiring = None

    def _poll_delay(self, poll_interval: float, attempt: int) -> float:
        # A single-file lock retries on a fixed cadence. A backend that publishes several files per acquisition sets a
        # cap, and then contending processes back off across a jittered, exponentially widening window instead of
        # colliding on every poll; poll_interval stays the floor so a lone waiter is still responsive.
        if not self._poll_backoff_cap:
            return poll_interval
        # Cap the exponent before doubling: under heavy contention attempt reaches the thousands, and 2**attempt would
        # overflow the float multiply long before the window itself stops growing past the cap.
        window = min(
            self._poll_backoff_cap, poll_interval * 2 ** min(attempt, _MAX_BACKOFF_EXPONENT)
        )  # pragma: needs hard-link
        return max(poll_interval, secrets.randbelow(int(window * 1_000_000) + 1) / 1_000_000)  # pragma: needs hard-link

    def _reconcile_failed_acquire(self, canonical: str) -> None:
        # An acquire that raised while still holding the native lock (a hook that failed and whose rollback could not
        # release) must keep the registry entry so a later release can retry the OS unlock; otherwise roll the counter
        # back. is_locked was already reconciled by whichever release ran.
        if self.is_locked:
            self._commit_acquire(canonical)
        else:
            self._undo_acquire()

    def _invoke_on_acquired(self) -> None:
        # The wrapper runs in the backend executor for async locks, preserving the callback's documented thread.
        if self._on_acquired is None or self._context.lock_counter != 1:
            return
        try:
            self._on_acquired(cast("int", self._context.lock_file_fd))
        except BaseException as callback_error:  # arbitrary caller code; roll back on any failure
            callback_context = callback_error.__context__
            try:
                self._release_with_fork_tracking()
            except BaseException as release_error:  # ruff:ignore[blind-except]  # both errors surface via the group below
                _raise_body_and_release(callback_error, release_error)
            callback_error.__context__ = callback_context
            raise

    def _acquire_with_fork_tracking(self) -> None:
        with _fork_transition(self):
            try:
                self._acquire()
            except BaseException as acquisition_error:
                self._rollback_failed_acquire(acquisition_error)
                raise
            try:
                self._register_context_descriptor()
            except BaseException as registration_error:  # pragma: needs fork
                self._rollback_failed_registration(registration_error)
                raise
        if self.is_locked:
            self._invoke_on_acquired()

    def _rollback_failed_acquire(self, acquisition_error: BaseException) -> None:
        if not self.is_locked:
            return
        registration_error: BaseException | None = None
        tracking_error: BaseException | None = None
        try:
            self._register_context_descriptor()
        except BaseException as error:  # ruff:ignore[blind-except]  # preserve registration and acquisition failures
            registration_error = error
            try:
                # Rollback may fail too; retain the fd so a child can close it without another identity probe.
                self._register_unverified_context_descriptor()
            except BaseException as error:  # ruff:ignore[blind-except]  # pragma: no cover - allocation/control-flow during fallback
                tracking_error = error
        try:
            self._release_with_fork_tracking()
        except BaseException as rollback_error:  # ruff:ignore[blind-except]  # preserve rollback and acquisition failures
            _raise_cleanup_errors(
                "lock acquisition cleanup failed",
                acquisition_error,
                registration_error,
                tracking_error,
                rollback_error,
            )
        if registration_error is not None:  # pragma: needs fork
            _raise_cleanup_errors(
                "lock acquisition cleanup failed", acquisition_error, registration_error, tracking_error
            )

    def _rollback_failed_registration(self, registration_error: BaseException) -> None:  # pragma: needs fork
        tracking_error: BaseException | None = None
        try:
            # Rollback may fail too; retain the fd so a child can close it without another identity probe.
            self._register_unverified_context_descriptor()
        except BaseException as error:  # ruff:ignore[blind-except]  # pragma: no cover - allocation/control-flow during fallback
            tracking_error = error
        try:
            self._release_with_fork_tracking()
        except BaseException as rollback_error:  # ruff:ignore[blind-except]  # preserve rollback and registration failures
            _raise_cleanup_errors(
                "descriptor registration cleanup failed", registration_error, tracking_error, rollback_error
            )
        if tracking_error is not None:  # pragma: no cover - requires failed in-memory fallback
            _raise_cleanup_errors("descriptor registration cleanup failed", registration_error, tracking_error)

    def _release_with_fork_tracking(self) -> None:
        with _fork_transition(self):
            try:
                self._release()
            except BaseException as release_error:
                # A backend unlocks first and clears the descriptor second. A signal handler that raises in between
                # leaves a hold the OS dropped. A failed system call alone shows the lock is still held, so it keeps it.
                if self._context.lock_file_fd is not None and not _is_system_call_failure(release_error):
                    with contextlib.suppress(Exception):
                        self._release()
                raise
            finally:
                self._unregister_released_descriptor()

    def _register_context_descriptor(self) -> None:
        if self._context.lock_file_fd is not None and self._context.lock_file_fd_token is None:
            self._context.lock_file_fd_token = _register_owned_descriptor(
                self._context.lock_file_fd,
                self._context.lock_file_fd_identity,
            )

    def _register_unverified_context_descriptor(self) -> None:
        # The rollback only reaches here still holding a descriptor it never managed to register.
        if self._context.lock_file_fd is not None and self._context.lock_file_fd_token is None:  # pragma: no branch
            self._context.lock_file_fd_token = _register_unverified_owned_descriptor(self._context.lock_file_fd)

    def _unregister_released_descriptor(self) -> None:
        if self._context.lock_file_fd is None:
            if (token := self._context.lock_file_fd_token) is not None:  # pragma: needs fork
                _unregister_owned_descriptor(token)
            self._context.lock_file_fd_token = None
            self._context.lock_file_fd_identity = None

    def _undo_acquire(self) -> None:
        """Roll back the counter after a failed acquire, dropping the registry entry once nothing holds the path."""
        self._context.lock_counter = max(0, self._context.lock_counter - 1)
        if self._context.lock_counter == 0:
            self._drop_registry_entry()
            self._context.claim_root = None

    def _pin_lock_path(self, canonical: str) -> None:
        # A path without a final component names no file. Its canonical form is a directory, and pinning that would
        # change the error the backend raises.
        self._context.claim_root = None if canonical.endswith(os.sep) else canonical

    def _commit_acquire(self, canonical: str) -> None:
        """Record this instance as the holder once the first acquire succeeds, so peers can detect the deadlock."""
        # Keyed on the registry rather than the counter: an on_acquired hook that acquires again bumps the counter
        # past 1 before this first commit runs, and must not leave the hold unregistered.
        if self._context.lock_file_registry is None:
            key = self._registry_key(canonical)
            # The holder scope is resolved once at commit so a later release from another flow drops the right entry.
            self._context.lock_file_key = key
            # A shared instance may be released from another thread, which must clear the entry in the acquirer's
            # registry: the releaser's own thread-local registry never held it.
            self._context.lock_file_registry = _registry.held
            _registry.held[key] = self._holder_id

    def _drop_registry_entry(self) -> None:
        """Forget the key owned by this hold without resolving a mutable path again."""
        # Ordered so a run an interrupt cuts short repeats safely: the entry goes only while it is still ours, and the
        # key outlives the registry it indexes, so a later commit sees no registry and records the hold afresh.
        key, registry = self._context.lock_file_key, self._context.lock_file_registry
        if registry is not None and registry.get(key) == self._holder_id:
            del registry[key]
        self._context.lock_file_registry = None
        self._context.lock_file_key = None

    def _commit_release(self) -> None:
        """Record the lock as fully released: reset the recursion counter and drop the deadlock-registry entry."""
        self._context.lock_counter = 0
        self._drop_registry_entry()
        self._context.claim_root = None

    def _try_break_expired_lock(self) -> None:
        """Remove the lock file once it is older than :attr:`lifetime`; only an existence lock accepts one."""

    def _check_give_up(
        self,
        *,
        blocking: bool,
        cancel_check: Callable[[], bool] | None,
        timeout: float,
        start_time: float,
    ) -> bool:
        lock_id, lock_filename = id(self), self.lock_file
        if blocking is False:
            _LOGGER.debug("Failed to immediately acquire lock %s on %s", lock_id, lock_filename)
            return True
        if cancel_check is not None and cancel_check():
            _LOGGER.debug("Cancellation requested for lock %s on %s", lock_id, lock_filename)
            return True
        if 0 <= timeout < time.perf_counter() - start_time:
            _LOGGER.debug("Timeout on acquiring lock %s on %s", lock_id, lock_filename)
            return True
        return False

    def _unprobeable_holder(self) -> str | None:  # ruff:ignore[no-self-use]  # existence locks override it
        return None

    @abstractmethod
    def _acquire(self) -> None:
        """If the file lock could be acquired, self._context.lock_file_fd holds the file descriptor of the lock file."""
        raise NotImplementedError

    @abstractmethod
    def _release(self) -> None:
        """Releases the lock and sets self._context.lock_file_fd to None."""
        raise NotImplementedError


def _resolve_read_write_timeout(timeout: float, *, blocking: bool) -> float:
    # 0 would still get SQLite's full busy timeout, and a truthy "false" would turn one attempt into a blocking wait.
    _check_blocking(blocking)
    if (timeout := _resolve_timeout(timeout, blocking=blocking)) < 0 and timeout != -1 and blocking:
        msg = f"timeout must be a non-negative number or -1, not {timeout!r}"
        raise ValueError(msg)
    return timeout


def _acquire_thread_lock(
    lock: Lock, *, blocking: bool, timeout: float, poll_interval: float, cancel_check: Callable[[], bool] | None
) -> bool:
    """Take a read-write lock's in-process lock; with *cancel_check*, ask it every *poll_interval* while waiting."""
    if not blocking:
        return lock.acquire(blocking=False)
    if cancel_check is None:
        # Lock.acquire raises OverflowError past TIMEOUT_MAX, which is 49.7 days on Windows.
        return lock.acquire() if timeout == -1 else lock.acquire(timeout=min(timeout, TIMEOUT_MAX))
    deadline = None if timeout == -1 else time.perf_counter() + timeout
    while not lock.acquire(
        timeout=poll_interval if deadline is None else min(poll_interval, max(deadline - time.perf_counter(), 0))
    ):
        if cancel_check() or (deadline is not None and time.perf_counter() >= deadline):
            return False
    return True


def _resolve_timeout(timeout: float, *, blocking: bool) -> float:
    """Validate an acquire ``timeout``; every negative value waits without a limit."""
    if math.isnan(timeout := _seconds("timeout", timeout)) and blocking:
        msg = "timeout must be a number of seconds, not nan"
        raise ValueError(msg)
    # The fail-fast deadlock check only sees a negative timeout as unlimited, so spell inf that way.
    return -1 if timeout == math.inf else timeout


@dataclass(slots=True, weakref_slot=True)
class _FinalizerState:
    """A finalized lock's state, held for the release that runs once the lock is unreachable."""

    state: dict[str, object] | None


#: Weak references whose callback finishes a singleton's finalizer once the cache no longer finds it.
_UNREACHABLE_LOCKS: Final[set[ref[BaseFileLock]]] = set()


def _finalize_unreachable(holder: ref[_FinalizerState], cls: type[BaseFileLock], reference: ref[BaseFileLock]) -> None:
    _UNREACHABLE_LOCKS.discard(reference)
    if (finalizer_state := holder()) is None or (state := finalizer_state.state) is None:
        return  # pragma: no cover  # only a collector freeing a resurrected lock as part of a cycle drops the holder
    # The lock's dictionary points back at the holder; that cycle ends here.
    finalizer_state.state = None
    # The release methods only read and write this state, so a fresh instance takes it over. Dropping that instance
    # runs __del__ again, and the cache no longer finds the lock.
    stand_in = object.__new__(cls)
    stand_in.__dict__ = state
    del stand_in


def _is_system_call_failure(error: BaseException) -> bool:
    # A failed system call sets errno. A bare TimeoutError raised by a signal handler is an OSError without one.
    if _is_exception_group(error):
        return all(map(_is_system_call_failure, cast("_ExceptionGroupProtocol", error).exceptions))
    return isinstance(error, OSError) and error.errno is not None


def _on_this_stack(frame: FrameType | None) -> bool:
    if frame is None:
        return False
    caller = inspect.currentframe()
    while caller is not None and caller is not frame:
        caller = caller.f_back
    return caller is not None


# acquire() returns this wrapper instead of self so entering the with-statement does not call __enter__ a second
# time; returning self would re-acquire the lock in BaseFileLock.__enter__ without a matching release (issue #37).
class AcquireReturnProxy(Generic[_LockT]):
    """A context-aware object that will release the lock file when exiting."""

    def __init__(self, lock: _LockT) -> None:
        self.lock: _LockT = lock

    def __enter__(self) -> _LockT:
        return self.lock

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        # Widen to the bound: ty narrows the else branch to the type variable minus BaseFileLock, not to the union rest.
        lock: Final = cast("BaseFileLock | ReadWriteLock | SoftReadWriteLock", self.lock)
        if isinstance(lock, BaseFileLock):
            lock._release_in_context(exc_value)  # ruff:ignore[private-member-access]  # forwards __exit__ to the owned lock's context release
        else:  # a reader/writer lock does not carry a context_error_policy
            lock.release()


@dataclass
class FileLockContext:
    """Holds the context for a ``BaseFileLock`` object."""

    # A separate class so ThreadLocalFileContext can make the whole context thread-local.

    lock_file: str
    timeout: float
    mode: int
    blocking: bool
    poll_interval: float

    #: The lock lifetime in seconds; ``None`` means the lock never expires.
    lifetime: float | None = None

    #: File descriptor from os.open for the lock file; not None while the lock is held.
    lock_file_fd: int | None = None

    #: Registry token for the descriptor owned by this thread's context.
    lock_file_fd_token: int | None = None

    #: Identity captured by a backend that already inspected the descriptor.
    lock_file_fd_identity: tuple[int, int] | None = None

    #: Descriptor opened by a backend but not yet committed as the held lock.
    pending_lock_file_fd: int | None = None

    #: Identity captured for a descriptor whose acquisition has not committed.
    pending_lock_file_fd_identity: tuple[int, int] | None = None

    #: Depth of nested acquisitions; the lock is released only when it returns to 0.
    lock_counter: int = 0

    #: Canonical registry key captured when the first physical acquisition commits.
    lock_file_key: Hashable | None = None

    #: The acquiring thread's deadlock registry holding :attr:`lock_file_key`.
    lock_file_registry: dict[Hashable, int] | None = None

    #: Claim pathnames this owner published, removed by name on release so no holder ever unlinks a peer's claim.
    owner_claim_paths: tuple[str, ...] = ()

    #: The frame of the :meth:`BaseFileLock.release` tearing this hold down, so a re-entrant call from a signal handler
    #: on the same thread returns instead of closing the descriptor a second time.
    releasing: FrameType | None = None

    #: The frame of an acquire still polling for this hold, so one a signal handler starts on this thread fails fast.
    acquiring: FrameType | None = None

    #: Canonical lock path, resolved when an acquisition starts and kept until its hold ends. A waiter on a relative
    #: path stays in the directory it started in, and the release finds the file there, after a chdir in between.
    claim_root: str | None = None

    #: The lease claim of this hold, kept with it so a cross-thread release stops the right heartbeat.
    lease_claim: _LeaseClaim | None = None


class ThreadLocalFileContext(FileLockContext, local):
    """
    A thread local version of the ``FileLockContext`` class.

    Each thread's state lives in an ordinary :class:`FileLockContext` at :attr:`current`, which the lock also keeps, so
    ``release(force=True)`` and the finalizer can unlock a hold whose thread exited.
    """

    current: FileLockContext

    # threading.local runs __init__ again with these arguments the first time each thread touches the context.
    def __init__(  # ruff:ignore[too-many-arguments]  # mirrors the FileLockContext fields set at construction
        self,
        *,
        lock_file: str,
        timeout: float,
        mode: int,
        blocking: bool,
        poll_interval: float,
        lifetime: float | None = None,
    ) -> None:
        self.current = FileLockContext(
            lock_file=lock_file,
            timeout=timeout,
            mode=mode,
            blocking=blocking,
            poll_interval=poll_interval,
            lifetime=lifetime,
        )


def _delegate_to_current(name: str) -> property:
    return property(
        lambda context: getattr(context.current, name),
        lambda context, value: setattr(context.current, name, value),
    )


for _field in fields(FileLockContext):
    setattr(ThreadLocalFileContext, _field.name, _delegate_to_current(_field.name))


@dataclass(frozen=True)
class _OwnedDescriptor:
    fd: int
    creator_pid: int
    device: int | None
    inode: int | None


class _ForkState:
    def __init__(self) -> None:
        self.gate = Condition(RLock())
        self.registry_lock = RLock()
        self.parameter_models_lock = RLock()
        self.transitions: dict[int, dict[int, _ForkDescriptorOwner | None]] = {}
        self.active_transitions = 0
        self.admission_closed = False
        self.fork_owner_depths: dict[int, int] = {}
        self.pinned_objects: dict[int, list[tuple[_ForkResettable, ...]]] = {}
        self.pinned_classes: dict[int, list[tuple[_ForkResettableClass, ...]]] = {}
        self.provisional_descriptor_tokens: dict[int, list[tuple[int, ...]]] = {}
        self.pid = os.getpid()

    def reset_synchronization(self) -> None:  # pragma: forked child
        self.gate = Condition(RLock())
        self.registry_lock = RLock()
        self.parameter_models_lock = RLock()
        self.transitions = {}
        self.active_transitions = 0
        self.admission_closed = False
        self.fork_owner_depths = {}
        self.pinned_objects = {}
        self.pinned_classes = {}
        self.provisional_descriptor_tokens = {}


_FORK_OBJECTS: Final[WeakValueDictionary[int, _ForkResettable]] = WeakValueDictionary()
_FORK_CLASSES: Final[WeakValueDictionary[int, _ForkResettableClass]] = WeakValueDictionary()
_OWNED_DESCRIPTORS: Final[dict[int, _OwnedDescriptor]] = {}
_DESCRIPTOR_TOKENS: Final[count[int]] = count()
_TRANSITION_TOKENS: Final[count[int]] = count()
_FORK_STATE: Final = _ForkState()
_FORK_AUDIT_EVENTS: Final[frozenset[str]] = frozenset({"os.fork", "os.forkpty"})
# CPython before 3.12 guards sys.settrace with one process-wide flag that stays set while audit hooks run, so a thread
# switch inside a Python hook makes a concurrent settrace raise. Coverage and debuggers call it on every thread start.
_SETTRACE_SURVIVES_AUDIT_HOOKS: Final[bool] = sys.implementation.name != "cpython" or sys.version_info >= (3, 12)


@cache
def _resolve_lazy_imports_before_shutdown() -> None:
    if hasattr(types, "LazyImportType"):  # pragma: >=3.15 cover
        atexit.register(_resolve_lazy_imports)


def _resolve_lazy_imports() -> None:  # pragma: >=3.15 cover
    # Interpreter shutdown clears sys.modules and sys.meta_path before it finalizes a lock still alive at exit, so a
    # lazy import (PEP 810) that the release path in __del__ reaches first can no longer load. Load them while it can.
    for name, module in list(sys.modules.items()):
        if name == "filelock" or name.startswith("filelock."):
            for attribute, value in list(vars(module).items()):
                if type(value) is types.LazyImportType:
                    getattr(module, attribute)


def _register_fork_hooks() -> None:
    if _REGISTER_AT_FORK is None:
        return  # pragma: lacks fork
    if _SETTRACE_SURVIVES_AUDIT_HOOKS:  # pragma: needs fork  # pragma: needs settrace-safe-audit-hooks
        sys.addaudithook(_audit_fork_safety)
    _REGISTER_AT_FORK(  # pragma: needs fork
        before=_pin_fork_objects,
        after_in_parent=_resume_parent_after_fork,
        after_in_child=_reset_child_after_fork,
    )


@contextmanager
def _fork_transition(descriptor_owner: _ForkDescriptorOwner | None = None) -> Generator[None]:
    # No fork follows exit, and a daemon thread stopped inside this bookkeeping holds its lock forever.
    if not _HAS_REGISTER_AT_FORK or sys.is_finalizing():
        yield  # pragma: lacks fork
        return  # pragma: lacks fork
    _ensure_current_process()  # pragma: needs fork
    creator_pid = os.getpid()  # pragma: needs fork
    # A generator holding this transition can be finalized on another thread, which must close the entering thread's.
    thread_id: Final = get_ident()  # pragma: needs fork
    token: Final = _enter_fork_transition(descriptor_owner, thread_id)  # pragma: needs fork
    try:  # pragma: needs fork
        yield
    finally:
        if os.getpid() == creator_pid:  # pragma: needs fork
            _leave_fork_transition(token, thread_id)


def _register_fork_object(instance: _ForkResettable) -> None:
    if not _HAS_REGISTER_AT_FORK:
        return  # pragma: lacks fork
    with _fork_transition(), _FORK_STATE.registry_lock:  # pragma: needs fork
        _FORK_OBJECTS[id(instance)] = instance
        _refresh_owner_pins()


def _register_fork_class(cls: _ForkResettableClass) -> None:
    if not _HAS_REGISTER_AT_FORK:
        return  # pragma: lacks fork
    with _fork_transition(), _FORK_STATE.registry_lock:  # pragma: needs fork
        _FORK_CLASSES[id(cls)] = cls
        _refresh_owner_pins()


def _register_owned_descriptor(fd: int, identity: tuple[int, int] | None = None) -> int | None:
    if not _HAS_REGISTER_AT_FORK:
        return None  # pragma: lacks fork
    with _fork_transition():  # pragma: needs fork
        if identity is None:
            stat_result = os.fstat(fd)
            identity = stat_result.st_dev, stat_result.st_ino
        return _record_owned_descriptor(fd, identity)


def _register_unverified_owned_descriptor(fd: int) -> int | None:
    if not _HAS_REGISTER_AT_FORK:
        return None  # pragma: lacks fork
    with _fork_transition():  # pragma: needs fork
        return _record_owned_descriptor(fd, None)


def _record_owned_descriptor(fd: int, identity: tuple[int, int] | None) -> int:  # pragma: needs fork
    with _FORK_STATE.registry_lock:
        token = next(_DESCRIPTOR_TOKENS)
        _OWNED_DESCRIPTORS[token] = _OwnedDescriptor(
            fd=fd,
            creator_pid=os.getpid(),
            device=None if identity is None else identity[0],
            inode=None if identity is None else identity[1],
        )
    return token


def _unregister_owned_descriptor(token: int) -> None:  # pragma: needs fork
    # No fork follows exit, and a daemon thread stopped inside this bookkeeping holds its lock forever.
    if sys.is_finalizing():  # pragma: no cover  # only exit reaches it, after coverage stops measuring
        return
    with _fork_transition(), _FORK_STATE.registry_lock:
        _OWNED_DESCRIPTORS.pop(token, None)


def _pin_fork_objects() -> None:  # pragma: needs fork
    _ensure_current_process()
    thread_id = get_ident()
    provisional_descriptor_tokens: list[int] = []
    # A signal handler that raises during the wait does not stop the fork. The after hooks pop one snapshot per
    # admission closed here, so the finally records the snapshot on failure too.
    try:
        with _FORK_STATE.gate:
            _FORK_STATE.fork_owner_depths[thread_id] = _FORK_STATE.fork_owner_depths.get(thread_id, 0) + 1
            _FORK_STATE.admission_closed = True
            while _FORK_STATE.active_transitions > len(_FORK_STATE.transitions.get(thread_id, ())):
                _FORK_STATE.gate.wait()
            transition_owners = tuple(_FORK_STATE.transitions.get(thread_id, {}).values())
        _verify_unverified_descriptors()
        owners = {id(owner): owner for owner in transition_owners if owner is not None}
        provisional_descriptor_tokens.extend(
            starmap(
                _snapshot_descriptor_for_fork,
                (descriptor for owner in owners.values() for descriptor in owner._descriptors_for_fork()),  # ruff:ignore[private-member-access]  # snapshots each owner's own fork descriptors
            )
        )
    finally:
        with _FORK_STATE.registry_lock:
            _FORK_STATE.provisional_descriptor_tokens.setdefault(thread_id, []).append(
                tuple(provisional_descriptor_tokens)
            )
            _FORK_STATE.pinned_objects.setdefault(thread_id, []).append(tuple(_FORK_OBJECTS.values()))
            _FORK_STATE.pinned_classes.setdefault(thread_id, []).append(tuple(_FORK_CLASSES.values()))


def _resume_parent_after_fork() -> None:  # pragma: needs fork
    thread_id = get_ident()
    try:
        with _FORK_STATE.registry_lock:
            for token in _FORK_STATE.provisional_descriptor_tokens[thread_id].pop():
                _OWNED_DESCRIPTORS.pop(token, None)
            _FORK_STATE.pinned_objects[thread_id].pop()
            _FORK_STATE.pinned_classes[thread_id].pop()
            if not _FORK_STATE.provisional_descriptor_tokens[thread_id]:  # pragma: needs fork
                del _FORK_STATE.provisional_descriptor_tokens[thread_id]
                del _FORK_STATE.pinned_objects[thread_id]
                del _FORK_STATE.pinned_classes[thread_id]
    finally:
        # No other code reopens admission. A failure above would leave the other threads waiting for good.
        with _FORK_STATE.gate:
            # A handler that raised before the depth was stored left no entry to decrement.
            if (depth := _FORK_STATE.fork_owner_depths.get(thread_id, 1)) <= 1:
                _FORK_STATE.fork_owner_depths.pop(thread_id, None)
            else:  # pragma: no cover - earlier at-fork callbacks may deadlock first
                _FORK_STATE.fork_owner_depths[thread_id] = depth - 1
            _FORK_STATE.admission_closed = bool(_FORK_STATE.fork_owner_depths)
            _FORK_STATE.gate.notify_all()


def _ensure_current_process() -> None:
    _reset_child_after_fork()


def _reset_child_after_fork() -> None:  # pragma: forked child
    if (pid := os.getpid()) == _FORK_STATE.pid:
        return
    thread_id = get_ident()
    pinned_objects = _FORK_STATE.pinned_objects.get(thread_id, [()])[-1]
    pinned_classes = _FORK_STATE.pinned_classes.get(thread_id, [()])[-1]
    descriptors: list[_OwnedDescriptor] = []
    for token, descriptor in tuple(_OWNED_DESCRIPTORS.items()):
        if descriptor.creator_pid != pid:
            descriptors.append(_OWNED_DESCRIPTORS.pop(token))
    _FORK_STATE.reset_synchronization()
    _FORK_STATE.pid = pid
    _INIT_PARAMETER_MODELS.clear()
    _detach_child_state(descriptors, pinned_objects, pinned_classes)


def _enter_fork_transition(descriptor_owner: _ForkDescriptorOwner | None, thread_id: int) -> int:  # pragma: needs fork
    with _FORK_STATE.gate:
        while (
            _FORK_STATE.admission_closed
            and thread_id not in _FORK_STATE.fork_owner_depths
            and thread_id not in _FORK_STATE.transitions
        ):
            _FORK_STATE.gate.wait()  # pragma: no cover - exercised in isolated interpreter
        token = next(_TRANSITION_TOKENS)
        _FORK_STATE.transitions.setdefault(thread_id, {})[token] = descriptor_owner
        _FORK_STATE.active_transitions += 1
    return token


def _leave_fork_transition(token: int, thread_id: int) -> None:  # pragma: needs fork
    with _FORK_STATE.gate:
        owned: Final = _FORK_STATE.transitions[thread_id]
        del owned[token]
        if not owned:
            del _FORK_STATE.transitions[thread_id]
        _FORK_STATE.active_transitions -= 1
        _FORK_STATE.gate.notify_all()


def _verify_unverified_descriptors() -> None:  # pragma: needs fork
    with _FORK_STATE.registry_lock:
        for token, descriptor in tuple(_OWNED_DESCRIPTORS.items()):
            if descriptor.creator_pid != os.getpid() or descriptor.device is not None:
                continue
            try:
                stat_result = os.fstat(descriptor.fd)
            except OSError:
                continue
            _OWNED_DESCRIPTORS[token] = _OwnedDescriptor(
                fd=descriptor.fd,
                creator_pid=descriptor.creator_pid,
                device=stat_result.st_dev,
                inode=stat_result.st_ino,
            )


def _snapshot_descriptor_for_fork(fd: int, identity: tuple[int, int] | None) -> int:  # pragma: needs fork
    if identity is None:  # pragma: needs fork
        try:
            stat_result = os.fstat(fd)
        except OSError:
            pass
        else:
            identity = stat_result.st_dev, stat_result.st_ino
    return _record_owned_descriptor(fd, identity)


def _detach_child_state(  # pragma: forked child
    descriptors: list[_OwnedDescriptor],
    pinned_objects: tuple[_ForkResettable, ...],
    pinned_classes: tuple[_ForkResettableClass, ...],
) -> None:
    for descriptor in descriptors:
        if descriptor.device is None:
            continue
        try:
            stat_result = os.fstat(descriptor.fd)
        except OSError:
            continue
        if (stat_result.st_dev, stat_result.st_ino) != (descriptor.device, descriptor.inode):
            continue
        with contextlib.suppress(OSError):
            os.close(descriptor.fd)
    for instance in pinned_objects:
        instance._reset_after_fork_in_child()  # ruff:ignore[private-member-access]  # resets each pinned instance in the fork child
    for cls in pinned_classes:
        cls._reset_class_after_fork()
    _registry.held.clear()


def _refresh_owner_pins() -> None:  # pragma: needs fork
    with _FORK_STATE.gate:
        fork_owner_thread_ids = tuple(_FORK_STATE.fork_owner_depths)
    if get_ident() not in fork_owner_thread_ids:  # pragma: no cover - at-fork callbacks disable tracing
        return
    objects = tuple(_FORK_OBJECTS.values())
    classes = tuple(_FORK_CLASSES.values())
    for thread_id in fork_owner_thread_ids:
        if object_snapshots := _FORK_STATE.pinned_objects.get(thread_id):  # pragma: needs fork
            object_snapshots[:] = [objects] * len(object_snapshots)
            _FORK_STATE.pinned_classes[thread_id][:] = [classes] * len(object_snapshots)


# The defaults capture the module globals: the hook outlives them at interpreter shutdown, where CPython wipes the
# module dict to None before the final audit events fire.
def _audit_fork_safety(  # pragma: no cover - CPython disables tracing while Python audit hooks run
    event: str,
    _args: Unused,
    *,
    _fork_events: frozenset[str] = _FORK_AUDIT_EVENTS,
    _state: _ForkState = _FORK_STATE,
    _get_ident: Callable[[], int] = get_ident,
) -> None:
    if event in _fork_events:
        if _get_ident() in _state.transitions or _state.fork_owner_depths:
            msg = f"{event} is unsafe while filelock is changing descriptor ownership"
            raise RuntimeError(msg)
    elif event == "_posixsubprocess.fork_exec" and _get_ident() in _state.transitions:
        msg = "fork_exec is unsafe while filelock is changing descriptor ownership"
        raise RuntimeError(msg)


_register_fork_hooks()


__all__ = [
    "_UNSET_FILE_MODE",
    "AcquireReturnProxy",
    "BaseFileLock",
    "CloseErrorPolicy",
    "ContextErrorPolicy",
    "FileLockContext",
    "FileLockMeta",
    "LockOptions",
    "ThreadLocalFileContext",
    "_acquire_thread_lock",
    "_append_exception_context",
    "_canonical",
    "_check_blocking",
    "_check_timeout_max",
    "_delegate_to_current",
    "_ensure_current_process",
    "_fork_transition",
    "_grouped_errors",
    "_raise_body_and_release",
    "_raise_chained_errors",
    "_raise_cleanup_errors",
    "_raise_grouped_errors",
    "_register_fork_class",
    "_register_fork_object",
    "_register_owned_descriptor",
    "_resolve_poll_interval",
    "_resolve_read_write_timeout",
    "_resolve_timeout",
    "_seconds",
    "_timeout_behavior",
    "_unregister_owned_descriptor",
]
