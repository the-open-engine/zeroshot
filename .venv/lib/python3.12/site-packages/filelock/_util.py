from __future__ import annotations

__lazy_modules__ = ["pathlib", "platform", "secrets"]

import errno
import os
import platform
import secrets
import stat
import sys
import time
from contextlib import contextmanager, suppress
from errno import EACCES, EINVAL, EIO, EISDIR, ENOSYS, EXDEV
from functools import cache
from pathlib import Path
from typing import TYPE_CHECKING, Final, cast

if TYPE_CHECKING:
    import ctypes
    from collections.abc import Callable, Generator
    from contextlib import AbstractContextManager


def write_all(fd: int, data: bytes) -> None:
    """
    Write the whole buffer to *fd*, looping over the short writes POSIX lets ``os.write`` make.

    A caller that stopped after one short write would leave a truncated marker behind. A peer that reads between two
    writes sees a prefix of the record; each caller has to tolerate that. Lock records need not survive a power loss, so
    we skip ``fsync``.

    :param fd: file descriptor open for writing.
    :param data: bytes to write in full.

    :raises OSError: if a write reports zero progress before the record is complete.

    """
    remaining = memoryview(data)
    while remaining:
        if (written := os.write(fd, remaining)) == 0:
            raise OSError(EIO, "os.write wrote 0 bytes before the record was complete")
        remaining = remaining[written:]


def raise_on_not_writable_file(filename: str) -> None:
    """
    Raise an exception if attempting to open the file for writing would fail.

    Separates files that can never be written from files that are writable but currently locked.

    :param filename: file to check

    :raises OSError: as if the file was opened for writing.

    """
    try:
        # lstat, not stat: settles exists-and-writable in one syscall, and a hostile symlink at the lock path would
        # make stat inspect the link target, letting an attacker turn a contended acquire into a misleading
        # PermissionError / IsADirectoryError and probe that target's attributes. The real open passes O_NOFOLLOW and
        # refuses the symlink anyway.
        file_stat = os.lstat(filename)
    except OSError:
        return  # does not exist, or an error the caller cannot act on

    if stat.S_ISDIR(file_stat.st_mode):
        if sys.platform == "win32":  # pragma: win32 cover
            raise PermissionError(EACCES, "Permission denied", filename)
        raise IsADirectoryError(EISDIR, "Is a directory", filename)  # pragma: win32 no cover

    # No mtime guard: the old `if st_mtime != 0` skip covered NFS/Linux quirks where os.lstat returned an all-zero
    # struct, which it no longer does. Skipping on mtime 0 let a read-only file or a directory at the lock path pass
    # as missing, so acquire() blocked forever on an open that cannot succeed.
    # Match open() credentials where supported; group permissions and ACLs can grant access without S_IWUSR. Do not
    # follow a symlink, as the lstat above does not: one swapped in since would be judged by its target.
    if not (file_stat.st_mode & stat.S_IWUSR) and not os.access(
        filename,
        os.W_OK,
        effective_ids=os.access in os.supports_effective_ids,
        follow_symlinks=os.access not in os.supports_follow_symlinks,
    ):
        try:
            os.lstat(filename)
        except FileNotFoundError:
            # A holder may unlink its marker between lstat() and access(); let the caller attempt creation.
            return
        raise PermissionError(EACCES, "Permission denied", filename)


def ensure_directory_exists(filename: Path | str) -> None:
    """
    Ensure the directory containing the file exists (create it if necessary).

    :param filename: file.

    """
    Path(filename).parent.mkdir(parents=True, exist_ok=True)


@contextmanager
def pin_inode(lock_file: str) -> Generator[int | None]:
    """
    Keep the file at *lock_file* allocated for the length of the block and yield its inode number.

    An inode is the kernel's identity for a file, independent of its name, and its number names a file only while the
    inode stays allocated. ext4 hands a freed number to the next file created. Once a stale marker is unlinked, the next
    holder's marker can therefore carry the number a contender read a moment ago. A caller that reads the marker inside
    the block, gives up unless it read the yielded inode, and breaks the marker before leaving can rely on the number.

    :param lock_file: path to the lock file about to be read and perhaps broken.

    :returns: the pinned inode number, or ``None`` where nothing can be pinned. Windows cannot rename a file this
        process holds open, so it needs no pin. Without ``O_PATH`` a symlink or a socket cannot be opened.

    :raises FileNotFoundError: if the path is gone.

    """
    if sys.platform == "win32":  # pragma: win32 cover
        yield None
    else:  # pragma: win32 no cover
        # O_PATH pins any node, a symlink or socket included, without read permission and without the filesystem seeing
        # an open; elsewhere a nonblocking read-only open pins what it can.
        flags: Final = getattr(os, "O_PATH", os.O_RDONLY | getattr(os, "O_NONBLOCK", 0)) | getattr(os, "O_NOFOLLOW", 0)
        try:
            fd = os.open(lock_file, flags)
        except FileNotFoundError:
            raise
        except OSError:
            yield None
            return
        try:
            yield os.fstat(fd).st_ino
        finally:
            os.close(fd)


def remove_own_file(lock_file: str, fd: int) -> None:  # pragma: win32 no cover
    """
    Remove *lock_file* while it still names the file open as *fd*; the caller closes the descriptor afterwards.

    Where the platform has an atomic exchange, a stale break takes the file through it, so a peer's marker found at the
    path goes back. Otherwise the path is unlinked by name if it still names this file. The open descriptor keeps the
    inode allocated, so a matching number cannot belong to a successor's marker.

    :raises OSError: if the descriptor cannot be read or the unlink fails.

    """
    own: Final = os.fstat(fd)
    with suppress(FileNotFoundError):
        break_lock_file(lock_file, own.st_mtime, own.st_ino)
        if os.path.samestat(os.lstat(lock_file), own):
            Path(lock_file).unlink()


def foreign_directory_owner(directory: os.stat_result, parent: os.stat_result) -> int | None:
    """
    Return the uid that owns *directory* when another account controls it under a sticky parent.

    A lock that keeps its ownership records in a directory relies on them staying in place. The sticky bit (a directory
    flag that lets only a file's owner remove it) on a parent such as ``/tmp`` stops other accounts from renaming the
    directory away. A directory that this account or the parent's owner owns is therefore safe there. One that another
    account owns is outside the lock's control. Without the sticky bit, any process that can create the directory can
    also rename this account's own one away, so a refusal there would change nothing. Linux applies the same rule for
    ``fs.protected_regular``.

    :param directory: ``lstat`` of the directory found in place.
    :param parent: ``stat`` of the directory that holds it.

    :returns: the owning uid when another account controls the directory, otherwise ``None``.

    """
    if sys.platform != "win32" and parent.st_mode & stat.S_ISVTX:  # pragma: needs foreign-sticky-tmp
        return None if directory.st_uid in {os.geteuid(), parent.st_uid} else directory.st_uid
    return None


def break_lock_file(lock_file: str, mtime_before: float, ino_before: int) -> None:
    """
    Remove the lock file a caller found stale with modification time *mtime_before* and inode *ino_before*.

    The caller keeps the judged file open until we return, through :func:`pin_inode` or its own descriptor, so no
    other file can carry *ino_before* meanwhile. Right before breaking we ``lstat`` the lock again and leave it alone
    if a peer recreated it since the caller judged it stale, as Mercurial re-reads a lock under its break lock.

    On Linux (``renameat2``) and macOS (``renamex_np``) the platform can exchange two names in one step. There we swap
    the marker with a placeholder of our own, so the lock path never stands empty, and a contender that polls meanwhile
    finds a file it cannot parse and waits. If the file we took is not the one judged stale, a peer recreated the lock,
    and a second exchange puts it back. Otherwise we remove it and then our placeholder. Of several processes racing to
    break the same lock, only one takes the stale file.

    Windows cannot exchange names, but it refuses to rename a marker its live holder keeps open. There we rename the
    marker aside and, if it was recreated after all, link it back. ``link`` never replaces a name, so a marker a third
    process created meanwhile keeps the path. Elsewhere without an exchange (NFS, SMB and other filesystems that refuse
    it, platforms without the call) a rename would leave the path empty beside a possibly live holder. We then do not
    move the marker, and the caller waits for ``timeout`` or ``break_lock()``.

    We compare inodes as well as modification times, because NFS and FAT store coarse modification times. The times are
    compared for equality, since a file's owner can set any time. We ``lstat`` so a symlink a peer swaps in is never
    followed. The break name carries a random token so no other process can guess it.

    :param lock_file: path to the lock file to break.
    :param mtime_before: modification time the caller saw when it judged the lock stale.
    :param ino_before: inode number the caller saw when it judged the lock stale.

    :raises OSError: if taking the marker fails, for example because the file vanished, another user owns it in a
        sticky directory, or on Windows its holder keeps it open.

    """
    if (current := os.lstat(lock_file)).st_mtime != mtime_before or current.st_ino != ino_before:
        return
    break_path: Final[str] = f"{lock_file}.break.{os.getpid()}.{secrets.token_hex(16)}"
    if exchange_supported():  # pragma: win32 no cover
        _swap_out(lock_file, break_path, mtime_before, ino_before)
    elif _MOVES_WITHOUT_EXCHANGE:  # pragma: win32 cover
        _break_by_rename(lock_file, break_path, mtime_before, ino_before)


def _swap_out(lock_file: str, break_path: str, mtime_before: float, ino_before: int) -> None:  # pragma: win32 no cover
    # An atomic exchange with a placeholder takes the marker without the lock path ever being empty. No contender can
    # then create a marker while this one is judged, and a live marker goes back the same way.
    fd: Final = os.open(break_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        try:
            # Not a record any lock writes, so a contender that reads it waits, and one left by a crash ages out. The
            # PID tells the contender that evicts it whose break names to search for a marker this process left.
            write_all(fd, f"filelock/break\n{os.getpid()}\n".encode())
            exchanged = exchange_names(break_path, lock_file)
        except OSError:
            exchanged = False
        if not exchanged:
            Path(break_path).unlink()
            return
        if (grabbed := os.lstat(break_path)).st_mtime == mtime_before and grabbed.st_ino == ino_before:
            Path(break_path).unlink()
            # Our descriptor keeps the placeholder's inode allocated, so a matching number at the path is still ours.
            if os.path.samestat(os.lstat(lock_file), os.fstat(fd)):
                Path(lock_file).unlink()
        else:
            exchange_back(lock_file, break_path, fd)
    finally:
        os.close(fd)


def exchange_back(lock_file: str, aside: str, fd: int) -> bool:  # pragma: win32 no cover
    # Raced breakers nest: each aside holds the prior marker. Returning out of order can strand a live holder's marker.
    # A peer may move the path between our check and exchange, so stalled breakers exchange to open any resulting ring.
    try:
        settled = _exchange_until_aside(lock_file, aside, os.fstat(fd))
    except FileNotFoundError:
        settled = True
    if settled:
        Path(aside).unlink(missing_ok=True)
    return settled


def _exchange_until_aside(lock_file: str, aside: str, own: os.stat_result) -> bool:  # pragma: win32 no cover
    # Peers need a few system calls to unwind; a paused or dead peer must not block the caller indefinitely.
    deadline: Final = time.monotonic() + 2.0
    stall: Final = 0.01
    feed_at = time.monotonic() + stall
    while not os.path.samestat(os.lstat(aside), own):
        now = time.monotonic()
        ours = os.path.samestat(os.lstat(lock_file), own)
        if not ours and now >= deadline:
            return False
        if ours or now >= feed_at:
            if not exchange_names(aside, lock_file):
                return False
            feed_at = now + stall
        else:
            time.sleep(0.0005)
    return True


def _break_by_rename(
    lock_file: str, break_path: str, mtime_before: float, ino_before: int
) -> None:  # pragma: win32 cover
    # Renaming the marker aside empties the path until a recreated live marker is put back. link never replaces a name,
    # so a marker a third process created meanwhile keeps the path, and the live one waits under the break name.
    # Without os.link, or where the filesystem refuses hard links, the live one waits there too.
    with _pinned_for_rename(lock_file):
        Path(lock_file).rename(break_path)
    if (st_after := os.lstat(break_path)).st_mtime == mtime_before and st_after.st_ino == ino_before:
        unlink_path(break_path)
    elif stat.S_ISREG(st_after.st_mode) and hasattr(os, "link"):  # pragma: needs hard-link
        with suppress(OSError):
            os.link(break_path, lock_file, follow_symlinks=False)
            unlink_path(break_path)


if sys.platform == "win32":  # pragma: win32 cover

    def _pinned_for_rename(path: str) -> AbstractContextManager[object]:
        # A rename by path waits on a Level 1 or batch oplock holder until it answers or closes. Opening first with
        # FILE_COMPLETE_IF_OPLOCKED turns that wait into BlockingIOError, and the open handle shares delete, so the
        # rename then proceeds. Imported here because _windows imports this module.
        from filelock._windows import open_for_delete_without_oplock_wait  # ruff:ignore[import-outside-top-level]

        return os.fdopen(open_for_delete_without_oplock_wait(path), "rb")

    def unlink_path(path: str) -> None:
        from filelock._windows import unlink_without_oplock_wait  # ruff:ignore[import-outside-top-level]

        unlink_without_oplock_wait(path)

else:  # pragma: win32 no cover

    def unlink_path(path: str) -> None:
        Path(path).unlink()


def exchange_supported() -> bool:
    """Whether this platform has a call that swaps two names in one step; a filesystem may still refuse it."""
    return _rename_call() is not None


def exchange_names(source: str, target: str) -> bool:  # pragma: win32 no cover
    """
    Swap the names *source* and *target* in one step, so neither ever stands empty, where :func:`exchange_supported`.

    :returns: ``False`` where the filesystem cannot, so the caller falls back

    :raises OSError: for any other failure, such as a missing name or a denied rename

    """
    renamer, exchange_flag = cast("tuple[Callable[[bytes, bytes, int], int], int]", _rename_call())
    if not (error := renamer(os.fsencode(source), os.fsencode(target), exchange_flag)):
        return True
    # EXDEV: overlayfs and some FUSE filesystems refuse an exchange between layers.
    if error in {EINVAL, ENOSYS, EXDEV, getattr(errno, "ENOTSUP", EINVAL), getattr(errno, "EOPNOTSUPP", EINVAL)}:
        return False
    raise OSError(error, os.strerror(error), source, None, target)


@cache
def _rename_call() -> tuple[Callable[[bytes, bytes, int], int], int] | None:
    # The call returns 0 or the errno. Looked up on first use, since most processes never break a lock and ctypes is
    # slow to import. None where ctypes is missing (GraalPy) or the platform has no exchange.
    try:
        import ctypes  # ruff:ignore[import-outside-top-level]  # only a stale break needs it
    except ImportError:  # pragma: no cover  # GraalPy ships without ctypes
        return None
    if sys.platform in {"linux", "android"}:  # pragma: linux cover
        if (renameat2 := _renameat2(ctypes.CDLL(None, use_errno=True))) is None:
            return None  # pragma: no cover  # an architecture the CI matrix does not run
        return renameat2, 2  # RENAME_EXCHANGE in linux/fs.h
    if sys.platform == "darwin":  # pragma: darwin cover
        renamex_np: Final = ctypes.CDLL(None, use_errno=True).renamex_np
        renamex_np.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        renamex_np.restype = ctypes.c_int

        def call_renamex_np(source: bytes, target: bytes, flags: int) -> int:
            return 0 if renamex_np(source, target, flags) == 0 else ctypes.get_errno()

        return call_renamex_np, 0x2  # RENAME_SWAP in sys/stdio.h
    return None  # pragma: linux no cover  # pragma: android no cover  # pragma: darwin no cover


def _renameat2(libc: ctypes.CDLL) -> Callable[[bytes, bytes, int], int] | None:  # pragma: linux cover
    # Reached through syscall(2) rather than the C library's wrapper. glibc 2.28, musl 1.2.2 and bionic API 30 added
    # the wrapper, and Termux ships an older bionic; the kernel has had the call since 3.15. The number is the
    # architecture's own, so an architecture outside the table gets no exchange rather than another call.
    import ctypes  # ruff:ignore[import-outside-top-level]  # only a stale break needs it

    numbers: Final = {
        "x86_64": 316,
        "i386": 353,
        "i686": 353,
        "armv7l": 382,
        "armv8l": 382,
        "aarch64": 276,
        "riscv64": 276,
        "loongarch64": 276,
        "ppc64le": 357,
        "s390x": 347,
    }
    if (number := numbers.get(platform.machine())) is None:
        return None  # pragma: no cover  # an architecture the CI matrix does not run
    syscall: Final = libc.syscall
    syscall.restype = ctypes.c_long
    at_fdcwd: Final = -100

    def call_renameat2(source: bytes, target: bytes, flags: int) -> int:
        arguments: Final = (
            ctypes.c_long(number),
            ctypes.c_int(at_fdcwd),
            ctypes.c_char_p(source),
            ctypes.c_int(at_fdcwd),
            ctypes.c_char_p(target),
            ctypes.c_uint(flags),
        )
        return 0 if syscall(*arguments) == 0 else ctypes.get_errno()

    return call_renameat2


def touch(name: str, *, fd: int) -> None:
    # Prefer the already-open, already-verified fd so a peer that swaps a symlink or a different file in at the
    # path after our O_NOFOLLOW read cannot redirect the touch: utime then targets the inode behind the fd.
    # Where the platform cannot utime an fd, fall back to a path-based touch that still refuses to follow a
    # symlink where supported, matching the O_NOFOLLOW reads used elsewhere here.
    if _SUPPORTS_UTIME_FD:  # pragma: needs utime-fd
        os.utime(fd, None)
        return
    os.utime(name, None, follow_symlinks=not _SUPPORTS_UTIME_NOFOLLOW)  # pragma: lacks utime-fd


# Windows refuses to rename a marker its live holder keeps open, so renaming aside only moves a marker nobody holds.
# Elsewhere without an exchange a contender could take the emptied path beside a live holder, so the marker stays and
# the lock counts as held.
_MOVES_WITHOUT_EXCHANGE: Final[bool] = sys.platform == "win32"
# Retargeting os.utime to an open fd lets a heartbeat refresh the exact inode it verified instead of whatever the
# pathname now names.
_SUPPORTS_UTIME_FD: Final[bool] = sys.platform != "win32" and os.utime in os.supports_fd
# os.utime follows symlinks unless told not to; not every platform can refuse the follow, so probe support.
_SUPPORTS_UTIME_NOFOLLOW: Final[bool] = os.utime in os.supports_follow_symlinks


__all__ = [
    "break_lock_file",
    "ensure_directory_exists",
    "exchange_back",
    "exchange_names",
    "exchange_supported",
    "foreign_directory_owner",
    "pin_inode",
    "raise_on_not_writable_file",
    "remove_own_file",
    "touch",
    "unlink_path",
    "write_all",
]
