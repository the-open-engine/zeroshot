"""
A platform independent file lock that supports the with-statement.

.. autodata:: filelock.__version__
    :no-value:

.. autodata:: filelock.OwnerMode

.. autodata:: filelock.CompromiseReason

"""

from __future__ import annotations

__lazy_modules__ = ["filelock._descriptor", "filelock._error", "importlib"]

import sys
import warnings
from importlib import import_module
from typing import TYPE_CHECKING, Final

from filelock._api import AcquireReturnProxy, BaseFileLock, CloseErrorPolicy, ContextErrorPolicy, LockOptions
from filelock._descriptor import lock_descriptor, unlock_descriptor
from filelock._error import LeaseSettingsMismatch, SoftFileLockLifetimeWarning, SoftFileLockProtocolError, Timeout
from filelock._unix import UnixFileLock
from filelock._unix import has_fcntl as _has_fcntl
from filelock._windows import WindowsFileLock

# Keeps ``filelock.version`` the string 4.0.12 exposed rather than the submodule.
from filelock.version import version

if TYPE_CHECKING:
    from filelock import asyncio
    from filelock._async_read_write import AsyncAcquireReadWriteReturnProxy, AsyncReadWriteLock
    from filelock._lease import CompromiseReason, LeaseCompromise, SoftFileLease
    from filelock._marker import MarkerSoftFileLock, OwnerMode, OwnerRecord
    from filelock._read_write import ReadWriteLock
    from filelock._soft import SoftFileLock
    from filelock._soft_rw import AsyncAcquireSoftReadWriteReturnProxy, AsyncSoftReadWriteLock, SoftReadWriteLock
    from filelock._strict import StrictSoftFileClaim, StrictSoftFileClaimState, StrictSoftFileLock
    from filelock.asyncio import (
        AsyncAcquireReturnProxy,
        AsyncSoftFileLease,
        AsyncSoftFileLock,
        AsyncStrictSoftFileLock,
        AsyncUnixFileLock,
        AsyncWindowsFileLock,
        BaseAsyncFileLock,
    )

#: version of the project as a string
__version__: Final[str] = version

#: ``True`` when :func:`fcntl.flock` is available. Off Windows, :data:`FileLock` then resolves to
#: :class:`UnixFileLock`, otherwise to :class:`SoftFileLock`. Always ``False`` on Windows.
#:
#: :meta hide-value:
has_fcntl: Final[bool] = _has_fcntl


if sys.platform == "win32":  # pragma: win32 cover
    _FileLock: type[BaseFileLock] = WindowsFileLock
else:  # pragma: win32 no cover # ruff:ignore[collapsible-else-if]  # the else carries the win32 no-cover pragma
    if has_fcntl:
        _FileLock: type[BaseFileLock] = UnixFileLock
    else:
        from filelock._soft import SoftFileLock

        _FileLock = SoftFileLock
        warnings.warn("only soft file lock is available", stacklevel=2)

if TYPE_CHECKING:
    FileLock = SoftFileLock
    AsyncFileLock = AsyncSoftFileLock
else:
    #: Alias for the lock, which should be used for the current platform.
    FileLock = _FileLock

# Importing asyncio and sqlite3 costs more than the rest of the package, so names that need them load on first access.
_LAZY_ATTRIBUTES: Final[dict[str, str]] = {
    "AsyncAcquireReadWriteReturnProxy": "._async_read_write",
    "AsyncAcquireReturnProxy": ".asyncio",
    "AsyncAcquireSoftReadWriteReturnProxy": "._soft_rw",
    "AsyncFileLock": ".asyncio",
    "AsyncReadWriteLock": "._async_read_write",
    "AsyncSoftFileLease": ".asyncio",
    "AsyncSoftFileLock": ".asyncio",
    "AsyncSoftReadWriteLock": "._soft_rw",
    "AsyncStrictSoftFileLock": ".asyncio",
    "AsyncUnixFileLock": ".asyncio",
    "AsyncWindowsFileLock": ".asyncio",
    "BaseAsyncFileLock": ".asyncio",
    "CompromiseReason": "._lease",
    "LeaseCompromise": "._lease",
    "MarkerSoftFileLock": "._marker",
    "OwnerMode": "._marker",
    "OwnerRecord": "._marker",
    "ReadWriteLock": "._read_write",
    "SoftFileLease": "._lease",
    "SoftFileLock": "._soft",
    "SoftReadWriteLock": "._soft_rw",
    "StrictSoftFileClaim": "._strict",
    "StrictSoftFileClaimState": "._strict",
    "StrictSoftFileLock": "._strict",
    "asyncio": ".asyncio",
}


def __getattr__(name: str) -> object:
    if (module_name := _LAZY_ATTRIBUTES.get(name)) is None:
        msg = f"module {__name__!r} has no attribute {name!r}"
        raise AttributeError(msg)
    try:
        module = import_module(module_name, __name__)
    except ImportError as error:
        # Only a missing sqlite3 module (the package, CPython's _sqlite3, PyPy's _sqlite3_cffi) leaves the read-write
        # locks None; a frozen application that left out a filelock module gets the error that names it.
        sqlite_missing = (error.name or "").startswith(("sqlite3", "_sqlite3"))
        if module_name not in {"._read_write", "._async_read_write"} or not sqlite_missing:
            raise
        value = None  # pragma: lacks sqlite3
    else:
        if name == "asyncio":
            value = module
        else:
            value = getattr(module, f"Async{_FileLock.__name__}" if name == "AsyncFileLock" else name)
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted({*globals(), *_LAZY_ATTRIBUTES})


__all__ = [
    "AcquireReturnProxy",
    "AsyncAcquireReadWriteReturnProxy",
    "AsyncAcquireReturnProxy",
    "AsyncAcquireSoftReadWriteReturnProxy",
    "AsyncFileLock",
    "AsyncReadWriteLock",
    "AsyncSoftFileLease",
    "AsyncSoftFileLock",
    "AsyncSoftReadWriteLock",
    "AsyncStrictSoftFileLock",
    "AsyncUnixFileLock",
    "AsyncWindowsFileLock",
    "BaseAsyncFileLock",
    "BaseFileLock",
    "CloseErrorPolicy",
    "CompromiseReason",
    "ContextErrorPolicy",
    "FileLock",
    "LeaseCompromise",
    "LeaseSettingsMismatch",
    "LockOptions",
    "MarkerSoftFileLock",
    "OwnerMode",
    "OwnerRecord",
    "ReadWriteLock",
    "SoftFileLease",
    "SoftFileLock",
    "SoftFileLockLifetimeWarning",
    "SoftFileLockProtocolError",
    "SoftReadWriteLock",
    "StrictSoftFileClaim",
    "StrictSoftFileClaimState",
    "StrictSoftFileLock",
    "Timeout",
    "UnixFileLock",
    "WindowsFileLock",
    "__version__",
    "asyncio",
    "has_fcntl",
    "lock_descriptor",
    "unlock_descriptor",
]
