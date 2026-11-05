from __future__ import annotations

__lazy_modules__ = ["contextlib", "filelock._util", "pathlib", "typing", "warnings"]

import os
import sys
import warnings
from contextlib import suppress
from errno import EACCES, EAGAIN, ENOSYS, EWOULDBLOCK
from pathlib import Path
from typing import Final, cast

from filelock._api import BaseFileLock
from filelock._util import remove_own_file

has_fcntl = False
if sys.platform == "win32":  # pragma: win32 cover

    class UnixFileLock(BaseFileLock):
        """Uses the :func:`fcntl.flock` to hard lock the lock file on unix systems."""

        def _acquire(self) -> None:
            raise NotImplementedError

        def _release(self) -> None:
            raise NotImplementedError

else:  # pragma: win32 no cover
    try:
        import fcntl

        _ = (fcntl.flock, fcntl.LOCK_EX, fcntl.LOCK_NB, fcntl.LOCK_UN)
    except (ImportError, AttributeError):
        _FCNTL_UNAVAILABLE: Final[str] = "fcntl is unavailable"

        def _lock_fd_nonblocking(_fd: int) -> bool:
            raise OSError(ENOSYS, _FCNTL_UNAVAILABLE)

        def _unlock_fd(_fd: int) -> None:
            raise OSError(ENOSYS, _FCNTL_UNAVAILABLE)

    else:
        has_fcntl = True
        # Contention errnos for a nonblocking flock. EAGAIN/EWOULDBLOCK are the usual "held elsewhere" codes; some
        # filesystems report EACCES instead, so treat it as contention too rather than a permanent error.
        _CONTENTION_ERRNOS: Final[frozenset[int]] = frozenset({EACCES, EAGAIN, EWOULDBLOCK})

        def _lock_fd_nonblocking(fd: int) -> bool:
            # One nonblocking exclusive flock attempt shared by UnixFileLock and lock_descriptor, so both contend on
            # the same lock and classify errors identically. The caller owns fd; this never closes it.
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as exception:
                if exception.errno in _CONTENTION_ERRNOS:
                    return False
                raise
            return True

        def _unlock_fd(fd: int) -> None:
            fcntl.flock(fd, fcntl.LOCK_UN)

    def _flock_is_missing(fd: int) -> bool:
        try:
            _lock_fd_nonblocking(fd)
        except OSError as exception:
            return exception.errno == ENOSYS
        return False  # a flock the probe won is released when the caller closes fd

    class UnixFileLock(BaseFileLock):
        """
        Uses the :func:`fcntl.flock` to hard lock the lock file on unix systems.

        We leave the lock file in place after release. Unlinking a locked file on Unix splits
        waiters across inodes and breaks mutual exclusion for processes that coordinate via the
        same path.
        """

        def _acquire(self) -> None:
            if (missing_flock := self._acquire_native()) is not None:
                fd, exception, created = missing_flock
                self._switch_to_soft_lock(fd, exception, created=created)

        def _acquire_native(self) -> tuple[int, OSError, bool] | None:
            if (opened := self._open_lock_file()) is None:
                return None
            fd, created = opened
            self._mark_descriptor_pending(fd)
            if created:
                # Before the flock: a peer may win it first, and only the creator may set the mode. Outside the flock
                # handler below: an fchmod that answers ENOSYS says nothing about flock.
                try:
                    self._apply_explicit_mode(fd)
                except BaseException:
                    self._mark_descriptor_released()
                    os.close(fd)
                    raise
            # One guard covers the flock through the recording of the hold. A signal handler that raises in between
            # would otherwise leave an unreferenced descriptor, and its flock, open until the process exits.
            locked = False
            try:
                locked = _lock_fd_nonblocking(fd)
                if locked and self._finalize_locked_fd(fd):
                    return None
            except BaseException as exception:
                if not locked and isinstance(exception, OSError) and exception.errno == ENOSYS:
                    return fd, exception, created
                # Contention returns False from _lock_fd_nonblocking, so a raise here is a failure.
                self._mark_descriptor_released()
                os.close(fd)
                raise
            self._mark_descriptor_released()
            os.close(fd)  # contention or an unlinked inode; let the retry loop try again
            return None

        def _open_lock_file(self) -> tuple[int, bool] | None:
            # No O_TRUNC: a contender that loses the flock must not truncate the holder's file (#591), and neither may
            # the winner, since a file renamed or hard-linked to the lock path is indistinguishable from a lock file.
            # O_EXCL tells whether this attempt created the inode: only then does it get the explicit mode, and the
            # ENOSYS fallback removes only its own placeholder. O_NONBLOCK: a lease (Linux F_SETLEASE) on the file would
            # otherwise block the open for up to fs.lease-break-time, beyond timeout, blocking=False and cancel_check.
            open_flags = os.O_RDWR | os.O_NONBLOCK
            if (o_nofollow := getattr(os, "O_NOFOLLOW", None)) is not None:
                open_flags |= o_nofollow
            open_mode = self._open_mode()
            try:
                return os.open(self._lock_path, open_flags | os.O_CREAT | os.O_EXCL, open_mode), True
            except FileExistsError:
                pass
            except FileNotFoundError:
                # On FUSE/NFS a split LOOKUP + CREATE lets a concurrent unlink() race the create, so ENOENT is transient
                # contention. An empty or separator-terminated path (Path() strips the separator) fails this way on
                # every attempt on macOS, so re-raise rather than retry forever.
                if self._lock_path and not self._lock_path.endswith(os.sep) and Path(self._lock_path).parent.exists():
                    return None
                raise
            except PermissionError:
                # Sticky-bit dirs (e.g. /tmp): O_CREAT fails if another user owns the file (#317).
                if not Path(self._lock_path).exists():
                    raise
            return self._open_existing_lock_file(open_flags, open_mode)

        def _open_existing_lock_file(self, open_flags: int, open_mode: int) -> tuple[int, bool] | None:
            try:
                return os.open(self._lock_path, open_flags, open_mode), False
            except FileNotFoundError:
                return None  # unlinked since the create attempt; let the retry loop start over
            except BlockingIOError:
                return None  # a lease is breaking: contention, so the retry loop keeps the deadline
            except PermissionError:
                # A marker another account left is readable but not writable. Without flock, waiting on it only needs
                # reading it; with flock the lock writes its file, so the denial stands.
                try:
                    fd = os.open(self._lock_path, open_flags & ~os.O_RDWR | os.O_RDONLY)
                except (FileNotFoundError, BlockingIOError):
                    return None  # unlinked, or leased: the kernel checks permission before the lease
                if _flock_is_missing(fd):
                    return fd, False
                os.close(fd)
                raise

        def _switch_to_soft_lock(self, fd: int, missing_flock: OSError, *, created: bool) -> None:
            # The filesystem does not implement flock. Fail closed when the caller opted out of existence-lock
            # semantics (#603), asked to preserve the pathname (#605), or set an on_acquired hook (#607), none of which
            # a soft lock can honor.
            falls_back: Final = self._fallback_to_soft and not self._preserve_lock_file and self._on_acquired is None
            try:
                # A file this attempt did not create may be a peer's live marker, so it stays. Our own placeholder goes
                # before the close: the open descriptor keeps its inode number from a peer's marker, and an exchange
                # puts back a peer's marker that replaced it.
                if created and falls_back:
                    with suppress(OSError):
                        remove_own_file(self._lock_path, fd)
            finally:
                self._mark_descriptor_released()
                os.close(fd)
            if not falls_back:
                raise missing_flock
            self._fallback_to_soft_lock()
            self._acquire()

        def _finalize_locked_fd(self, fd: int) -> bool:
            # Runs with the flock held; the caller closes fd on failure. A concurrent _release() may have unlinked
            # the file between our open() and flock(), which st_nlink 0 shows. That lock guards no name, so report
            # it and let the retry loop start over.
            if (stat_result := os.fstat(fd)).st_nlink == 0:
                return False
            self._mark_descriptor_owned(fd, (stat_result.st_dev, stat_result.st_ino))
            return True

        def _apply_explicit_mode(self, fd: int) -> None:
            if self.has_explicit_mode:
                with suppress(PermissionError):
                    os.fchmod(fd, self._context.mode)

        def _fallback_to_soft_lock(self: BaseFileLock) -> None:
            # Import lazily: this runs only on the rare flock fallback, and asyncio imports _unix, so a
            # module-level import of it here would cycle.
            from filelock._soft import SoftFileLock  # ruff:ignore[import-outside-top-level]

            warnings.warn("flock not supported on this filesystem, falling back to SoftFileLock", stacklevel=2)
            from filelock.asyncio import AsyncSoftFileLock, BaseAsyncFileLock  # ruff:ignore[import-outside-top-level]

            self.__class__ = AsyncSoftFileLock if isinstance(self, BaseAsyncFileLock) else SoftFileLock

        def _release(self) -> None:
            fd = cast("int", self._context.lock_file_fd)
            # Retain the descriptor until flock succeeds: a failed unlock leaves the kernel lock held, so is_locked
            # must keep reporting held for a retry. Once flock commits, clear held state and close as post-unlock
            # cleanup; a close failure (EIO on FUSE/Docker bind mounts) does not make the kernel lock held again.
            _unlock_fd(fd)
            self._mark_descriptor_released()
            self._close_released_fd(fd, default_suppresses=True)


if sys.platform == "win32":  # pragma: win32 cover
    __all__ = ["UnixFileLock", "has_fcntl"]
else:  # pragma: win32 no cover
    __all__ = ["UnixFileLock", "_lock_fd_nonblocking", "_unlock_fd", "has_fcntl"]
