"""The :class:`~._protocol.Files` implementation over :mod:`os`."""

from __future__ import annotations

__lazy_modules__ = ["filelock._soft_rw._protocol", "filelock._strict", "filelock._util", "pathlib"]

import errno
import os
import stat
import sys
import time
from contextlib import contextmanager, suppress
from errno import ELOOP, EMLINK, ENOENT, ENOTDIR, ENXIO, ESTALE
from pathlib import Path
from typing import TYPE_CHECKING, Final

from filelock._soft_rw._protocol import (
    _GENERATIONS_DIRECTORY,
    _HOLDERS_DIRECTORY,
    _MAX_RECORD_SIZE,
    TreeReplacedError,
)
from filelock._strict import _link_no_follow, _pin_directories, _raise_if_hard_links_unsupported
from filelock._util import ensure_directory_exists, foreign_directory_owner, write_all

if TYPE_CHECKING:
    from collections.abc import Generator

_O_NOFOLLOW: Final[int] = getattr(os, "O_NOFOLLOW", 0)
#: Probed once at import, as :mod:`filelock._strict` does, because a per-call check flips when a test mocks the
#: function. The set lists os.rename, whose implementation os.replace shares.
_SUPPORTS_DIR_FD: Final[bool] = (
    hasattr(os, "link")
    and {os.link, os.open, os.stat, os.mkdir, os.rename, os.unlink} <= os.supports_dir_fd
    and os.scandir in os.supports_fd
)
#: Windows opens descriptors in text mode by default, which rewrites newlines; every record here is exact bytes.
_O_BINARY: Final[int] = getattr(os, "O_BINARY", 0)
#: ESTALE is what an NFS client reports for a file a peer unlinked out from under its cached handle: gone, not broken.
_MISSING_ERRNOS: Final[frozenset[int]] = frozenset({ENOENT, ESTALE})
#: Windows refuses an open with EACCES while another process is deleting the file or holds it open without sharing;
#: both clear within moments, so an open is retried this long before the refusal is taken as real.
_WINDOWS_OPEN_GRACE: Final[float] = 0.5


class OsFiles:
    """Record files under a lock's protocol root, opened without following symlinks and owned by this UID alone."""

    def __init__(self, lock_file: str) -> None:
        self._lock_file = lock_file
        self._prepared = False
        self._identities: dict[str, tuple[int, int]] = {}

    def read(self, path: str) -> bytes | None:
        # O_NONBLOCK keeps an open of a FIFO planted at the path from blocking; the regular-file check below rejects it.
        if (fd := self._open(path, os.O_RDONLY | _O_NOFOLLOW | getattr(os, "O_NONBLOCK", 0) | _O_BINARY)) is None:
            return None
        try:
            if not stat.S_ISREG(os.fstat(fd).st_mode):
                return b""
            chunks: list[bytes] = []
            remaining = _MAX_RECORD_SIZE + 1
            while remaining > 0 and (chunk := os.read(fd, remaining)):
                chunks.append(chunk)
                remaining -= len(chunk)
            return b"".join(chunks)
        finally:
            os.close(fd)

    def create(self, path: str, data: bytes, *, durable: bool = False) -> tuple[int, int]:
        with self._relative(path) as (name, dir_fd):
            fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | _O_NOFOLLOW | _O_BINARY, 0o600, dir_fd=dir_fd)
        try:
            write_all(fd, data)
            if durable:
                os.fsync(fd)
            created = os.fstat(fd)
        except BaseException:
            os.close(fd)
            # The name is unique to this call, so removing it cannot take a record from anyone else.
            self.unlink(path)
            raise
        os.close(fd)
        return created.st_dev, created.st_ino

    def link(self, source: str, target: str) -> bool:
        failure: NotImplementedError | OSError | None = None
        with self._relative(source) as (source_name, source_fd), self._relative(target) as (target_name, target_fd):
            try:
                _link_no_follow(source_name, target_name, src_dir_fd=source_fd, dst_dir_fd=target_fd)
            except FileExistsError:
                pass
            except (NotImplementedError, OSError) as error:
                _raise_if_hard_links_unsupported(self._lock_file, error)
                failure = error
            # The file's identity decides, not the call's result: an NFS retransmit can report a link that landed as
            # failed, and an EEXIST can be this very link answered twice. Any other error on a link that did not land is
            # a fault, not a peer winning the race, and a retry would spin.
            if (identity := _identity(source_name, source_fd)) is not None and identity == _identity(
                target_name, target_fd
            ):
                return True
        if failure is not None:
            raise failure
        return False

    def replace(self, source: str, target: str) -> bool:
        try:
            with self._relative(source) as (source_name, source_fd), self._relative(target) as (target_name, target_fd):
                os.replace(source_name, target_name, src_dir_fd=source_fd, dst_dir_fd=target_fd)
        except PermissionError:
            # Windows refuses to rename over a file a reader holds open; callers tolerate a target one rename behind.
            return False
        return True

    def overwrite(self, path: str, data: bytes, identity: tuple[int, int]) -> bool:
        # O_NONBLOCK keeps an open of a FIFO (a named pipe) at the path from blocking, as in read. With no reader the
        # open fails with ENXIO.
        try:
            fd = self._open(path, os.O_WRONLY | _O_NOFOLLOW | getattr(os, "O_NONBLOCK", 0) | _O_BINARY)
        except OSError as error:
            if error.errno != ENXIO:
                raise
            return False  # pragma: needs fifo
        if fd is None:
            return False
        try:
            # O_NOFOLLOW refuses a symlink but not a hard link (a second name for the same file) or another file renamed
            # over the name, and a write through either lands in a file that is not the record. The protocol does not
            # link holder records, so a second name marks a file that inherited a removed record's inode number.
            opened = os.fstat(fd)
            if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != identity or opened.st_nlink != 1:
                return False
            write_all(fd, data)
        finally:
            os.close(fd)
        return True

    def unlink(self, path: str) -> None:
        try:
            with self._relative(path) as (name, dir_fd):
                os.unlink(name, dir_fd=dir_fd)
        except OSError as error:
            # Windows refuses to remove a file another process holds open; the record then outlives its owner until
            # that process exits, which only delays the cleanup a later participant repeats.
            if error.errno not in _MISSING_ERRNOS and not isinstance(error, PermissionError):
                raise

    def listdir(self, path: str) -> list[str]:
        try:
            with self._directory(path) as dir_fd, os.scandir(path if dir_fd is None else dir_fd) as entries:
                return [entry.name for entry in entries]
        except FileNotFoundError:
            return []

    def prepare(self, root: str, *, force: bool = False) -> None:
        if self._prepared and not force:
            return
        if force:
            self._identities.clear()
        ensure_directory_exists(root)
        for directory, depth in (
            (Path(root), 0),
            (Path(root, _GENERATIONS_DIRECTORY), 1),
            (Path(root, _HOLDERS_DIRECTORY), 1),
        ):
            with self._relative(str(directory), depth) as (name, dir_fd):
                with suppress(FileExistsError):
                    os.mkdir(name, 0o700, dir_fd=dir_fd)
                # mkdir has no O_NOFOLLOW, so check what the name resolves to before the lock creates anything in it.
                found = os.stat(name, dir_fd=dir_fd, follow_symlinks=False)
                parent = directory.parent.stat() if dir_fd is None else os.fstat(dir_fd)
            if (
                not stat.S_ISDIR(found.st_mode)
                or getattr(found, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT
            ):
                msg = f"{directory} exists but is not a directory or is a symlink; refusing to use it"
                raise TreeReplacedError(msg)
            owner = foreign_directory_owner(found, parent)
            if owner is not None:  # pragma: needs foreign-sticky-tmp
                msg = f"{directory} belongs to uid {owner}, not to this process's account or the parent's owner"
                raise TreeReplacedError(msg)
            self._identities[str(directory)] = found.st_dev, found.st_ino
        self._prepared = True

    def _open(self, path: str, flags: int) -> int | None:
        # Missing is the one answer a caller acts on without raising. A refused open is real on POSIX, where nothing
        # this lock does makes its own records unreadable; on Windows it is usually another process mid-delete or
        # holding the file without sharing, so it is retried through a short grace before it counts, and a file still
        # refusing after that has been in delete-pending for longer than any deletion takes, so it is on its way out:
        # missing.
        deadline: float | None = None
        while True:
            if isinstance(opened := self._attempt_open(path, flags), int):
                return opened
            if opened.errno in _MISSING_ERRNOS:
                return None
            if not isinstance(opened, PermissionError) or sys.platform != "win32":
                raise opened
            if deadline is None:  # pragma: win32 cover
                deadline = time.monotonic() + _WINDOWS_OPEN_GRACE
            elif time.monotonic() >= deadline:  # pragma: win32 cover
                return None
            time.sleep(0.002)  # pragma: win32 cover

    def _attempt_open(self, path: str, flags: int) -> int | OSError:
        try:
            with self._relative(path) as (name, dir_fd):
                return os.open(name, flags, dir_fd=dir_fd)
        except OSError as error:
            return error

    @contextmanager
    def _relative(self, path: str, depth: int = 2) -> Generator[tuple[str, int | None]]:
        directory, name = os.path.split(path)
        with self._directory(directory, depth) as dir_fd:
            yield (path if dir_fd is None else name), dir_fd

    @contextmanager
    def _directory(self, path: str, depth: int = 2) -> Generator[int | None]:
        # O_NOFOLLOW applies to the last path component. The check prepare() made on <root> and <root>/gen holds until a
        # process that can write the lock's directory renames the tree aside and puts one with a symlinked gen in its
        # place. Each call therefore opens the root and then the record's directory without following symlinks and
        # works inside what it opened. No descriptor outlives the call, so a later call finds a log removed and started
        # again by name, as before. None means descriptors cannot anchor a call and the call uses the whole path.
        if depth and _SUPPORTS_DIR_FD:  # pragma: needs link-dir-fd
            with self._relative(path, depth - 1) as (name, parent_fd):
                try:
                    fd = os.open(name, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | _O_NOFOLLOW, dir_fd=parent_fd)
                except OSError as error:
                    # NetBSD uses EFTYPE and FreeBSD uses EMLINK when O_NOFOLLOW refuses a symlink.
                    if error.errno not in {ELOOP, EMLINK, ENOTDIR, getattr(errno, "EFTYPE", ELOOP)}:
                        raise
                    msg = f"{path} is not a directory or is a symlink; refusing to use it"
                    raise TreeReplacedError(msg) from error
                try:
                    self._admit(fd, path, parent_fd)
                except BaseException:
                    os.close(fd)
                    raise
            try:
                yield fd
            finally:
                os.close(fd)
        else:
            # Without dir_fd, hold each of those directories open instead, so no process can rename or replace one.
            try:
                pins = _pin_directories(*[Path(path), *Path(path).parents][:depth][::-1])
            except NotADirectoryError as error:  # pragma: win32 cover
                msg = f"{error.filename} is not a directory or is a symlink; refusing to use it"
                raise TreeReplacedError(msg) from error
            with pins:
                yield None

    def _admit(self, fd: int, path: str, parent_fd: int | None) -> None:  # pragma: needs link-dir-fd
        # A directory other than the one prepare() checked may belong to another account: the tree can be renamed aside
        # and recreated while this object lives, and the ownership check has to hold for what is opened now.
        found = os.fstat(fd)
        if (found.st_dev, found.st_ino) == self._identities.get(path):
            return
        parent = os.fstat(parent_fd) if parent_fd is not None else Path(path).parent.stat()
        if (owner := foreign_directory_owner(found, parent)) is not None:  # pragma: needs foreign-sticky-tmp
            msg = f"{path} belongs to uid {owner}, not to this process's account or the parent's owner"
            raise TreeReplacedError(msg)
        self._identities[path] = found.st_dev, found.st_ino


def _identity(path: str, dir_fd: int | None) -> tuple[int, int] | None:
    try:
        st = os.stat(path, dir_fd=dir_fd, follow_symlinks=False)
    except OSError:
        return None
    return st.st_dev, st.st_ino


__all__ = [
    "OsFiles",
]
