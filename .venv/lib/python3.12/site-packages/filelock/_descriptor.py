"""A minimal native lock over a caller-owned file descriptor, contending with :class:`FileLock` on the same file."""

from __future__ import annotations

__lazy_modules__ = ["filelock._unix", "threading", "typing"]

import sys
import time
from threading import TIMEOUT_MAX
from typing import Final

from filelock._api import _seconds

if sys.platform == "win32":  # pragma: win32 cover
    from filelock._windows import _lock_fd_nonblocking, _unlock_fd
else:  # pragma: win32 no cover
    from filelock._unix import _lock_fd_nonblocking, _unlock_fd


def lock_descriptor(fd: int, *, blocking: bool = True, poll_interval: float = 0.05) -> bool:
    """
    Take the native OS lock on *fd*, a file descriptor the caller opened and owns.

    This is the same exclusive lock the native :class:`FileLock` backend uses: a whole-file :func:`fcntl.flock` lock
    on Unix, or a ``LockFileEx`` lock of one byte at offset 0 on Windows. A descriptor lock and a native path lock on
    the same file contend with each other. There are no offset or length parameters; seeking *fd* does not change
    the locked range. Interoperating with another tool requires agreement on the locking primitive, range, and protocol.

    Unlike :class:`FileLock` it adds no path handling: it never opens, truncates, closes, unlinks, chmods,
    canonicalizes, or falls back. The caller owns *fd* before, during, and after the call, and must close it.
    On Windows *fd* must be a synchronous descriptor (its handle not opened with ``FILE_FLAG_OVERLAPPED``).

    For timeout, reentrancy, singleton, lifetime, or stale-break behavior, use :class:`FileLock`. There is no async
    wrapper. Run this in an executor, or drive ``blocking=False`` from your own polling loop.

    :param fd: an open file descriptor the caller owns.
    :param blocking: when ``True`` (default), retry the nonblocking attempt every *poll_interval* seconds until it
        succeeds; when ``False``, make one attempt.
    :param poll_interval: positive seconds between attempts while blocking, at most :data:`threading.TIMEOUT_MAX`;
        ignored when *blocking* is ``False``.

    :returns: ``True`` once the lock is held, or ``False`` on contention when ``blocking`` is ``False``.

    :raises OSError: for a permanent native failure, such as an invalid descriptor, or with ``errno.ENOSYS`` when the
        Python build lacks the native locking primitive. The descriptor is left open.
    :raises ValueError: if a blocking call receives a *poll_interval* outside that range, including ``nan``.

    .. versionadded:: 3.30.0

    """
    if not blocking:
        return _lock_fd_nonblocking(fd)
    # time.sleep raises OverflowError past TIMEOUT_MAX; nan fails both comparisons.
    if not 0 < (poll_interval := _seconds("poll_interval", poll_interval)) <= TIMEOUT_MAX:
        msg: Final[str] = f"poll_interval must be greater than 0 and at most {TIMEOUT_MAX}, got {poll_interval}"
        raise ValueError(msg)
    while not _lock_fd_nonblocking(fd):
        time.sleep(poll_interval)
    return True


def unlock_descriptor(fd: int) -> None:
    """
    Release the native OS lock on *fd* without touching the descriptor.

    :param fd: the descriptor a prior :func:`lock_descriptor` locked; the caller still owns and must close it.

    :raises OSError: if the native unlock fails, including ``errno.ENOSYS`` when the Python build lacks the native
        locking primitive; the caller may retry on the same descriptor.

    .. versionadded:: 3.30.0

    """
    _unlock_fd(fd)


__all__ = [
    "lock_descriptor",
    "unlock_descriptor",
]
