from __future__ import annotations

__lazy_modules__ = [
    "contextlib",
    "filelock._identity",
    "filelock._soft_protocol",
    "filelock._util",
    "itertools",
    "pathlib",
    "re",
    "secrets",
    "typing",
]

import os
import re
import secrets
import stat
import sys
import threading
import time
from contextlib import suppress
from errno import EACCES, EEXIST, EPERM
from itertools import islice
from pathlib import Path
from typing import Final

from filelock._api import BaseFileLock, _raise_grouped_errors
from filelock._identity import host_name, owner_is_current_process, owner_is_stale, process_start_token
from filelock._soft_protocol import STRICT_SOFT_SENTINEL_RECORD
from filelock._util import (
    break_lock_file,
    exchange_back,
    exchange_names,
    exchange_supported,
    pin_inode,
    raise_on_not_writable_file,
    remove_own_file,
    write_all,
)

if sys.platform == "win32":  # pragma: win32 cover
    from filelock._windows import open_read_without_oplock_wait
    from filelock._windows import unlink_without_oplock_wait as unlink_path
else:  # pragma: win32 no cover
    from filelock._util import unlink_path

MALFORMED_LOCK_AGE_THRESHOLD: Final[float] = 2.0
_MAX_LOCK_FILE_SIZE: Final[int] = 1024
_UNLINK_MAX_RETRIES: Final[int] = 10
_MARKER_WITH_START_TOKEN_LINE_COUNT: Final[int] = 3
#: Contenders act on the record in the marker, so no other account may write it, whatever the mode or umask.
_OTHERS_WRITE: Final[int] = stat.S_IWGRP | stat.S_IWOTH
#: All a SoftFileLock before 3.22 writes, and what a current one shows between creating its marker and writing it.
_RECORDLESS_MARKER: Final[str] = ""
_CLAIMS_IN_PROGRESS: Final[set[str]] = set()
_CLAIMS_LOCK: Final[threading.Lock] = threading.Lock()


class SoftFileLock(BaseFileLock):
    """
    Cooperative file lock based on a shared existence marker.

    Unlike :class:`UnixFileLock <filelock.UnixFileLock>` and :class:`WindowsFileLock <filelock.WindowsFileLock>`, this
    lock does not use OS-level locking primitives. Instead, it creates the lock file with ``O_CREAT | O_EXCL`` and
    treats its existence as the lock indicator. The filesystem must provide coherent exclusive creation and directory
    updates to each participating process. A crash can leave the marker behind.

    The marker contains the holder's PID and hostname. A contender may remove it when it can no longer find a same-host
    process with that PID. A configured :attr:`~filelock.BaseFileLock.lifetime` also permits removal based on marker
    age, including while the holder remains alive. Age-based expiry can overlap protected operations and does not
    provide strict mutual exclusion.

    """

    #: Existence locks reclaim by unlinking a pathname, so an age-based lease may break one; a native inode lock cannot.
    _lifetime_supported: bool = True

    #: Age-based expiry preserves historical behavior but does not provide strict mutual exclusion.
    _lifetime_replacements: tuple[str, str] | None = ("StrictSoftFileLock", "SoftFileLease")

    #: An existence lock unlinks its marker to release, so it cannot promise to keep the pathname.
    _preserve_lock_file_supported: bool = False

    #: An existence lock keeps protocol state in its marker, so it cannot lend the descriptor to an on_acquired hook.
    _on_acquired_supported: bool = False

    #: The marker's (mtime, inode, ctime) when this instance last saw it change, and when, on its monotonic clock.
    _marker_seen: tuple[tuple[float, int, int], float] | None = None

    #: The identity of a marker a Windows release relinquished but has not unlinked yet.
    _marker_cleanup: tuple[int, int] | None = None

    #: The descriptor of that marker while its close has not run yet, as a list so one C call can take it.
    _marker_fds: list[int]

    def _acquire(self) -> None:
        self._finish_marker_cleanup()
        raise_on_not_writable_file(self._lock_path)
        # O_CREAT | O_EXCL makes the create fail with EEXIST when the file already exists, so a successful open
        # means this process now holds the lock.
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_TRUNC
        if (o_nofollow := getattr(os, "O_NOFOLLOW", None)) is not None:  # pragma: needs o-nofollow
            flags |= o_nofollow
        try:
            fd = os.open(self._lock_path, flags, self._open_mode() & ~_OTHERS_WRITE)
        except OSError as exception:
            if not (
                exception.errno == EEXIST or (exception.errno == EACCES and sys.platform == "win32")
            ):  # pragma: win32 no cover
                raise
            self._try_break_stale_lock()
            return
        # Publish held state only once the record is on disk. On failure, cancellation included, close the descriptor
        # and unlink the path only while it still names the file we opened: after an expired lease a successor's
        # marker may sit at the same path. The guard starts at the open, so a signal handler that raises before
        # _mark_descriptor_owned leaves no marker behind.
        identity: tuple[int, int] | None = None
        try:
            identity = _file_identity(os.fstat(fd))
            self._mark_descriptor_pending(fd)
            self._apply_explicit_mode(fd)
            self._write_lock_info(fd)
            self._mark_descriptor_owned(fd, identity)
        except BaseException:
            self._mark_descriptor_released()
            try:
                if identity is None:
                    # The first read failed, as a network mount reporting EIO or an interrupt landing mid-call does, and
                    # this attempt still owns the descriptor it created the marker through, so a retry usually answers.
                    # Without the identity the Windows unlink below cannot recognize the marker, and the empty marker it
                    # leaves blocks the path for good, since no contender breaks one.
                    with suppress(OSError):
                        identity = _file_identity(os.fstat(fd))
                if sys.platform != "win32":  # pragma: win32 no cover
                    # Before the close: our open descriptor keeps the inode allocated, so a successor's marker cannot
                    # carry its number.
                    with suppress(OSError):
                        remove_own_file(self._lock_path, fd)
            finally:
                try:
                    os.close(fd)
                finally:
                    if sys.platform == "win32":  # pragma: win32 cover
                        self._unlink_held_marker(identity)
            raise

    def _apply_explicit_mode(self, fd: int) -> None:
        # os.open filters the mode through the umask, so an explicit mode lands as given only through fchmod, the way
        # UnixFileLock applies it. Windows maps the mode to a read-only bit that os.open already set.
        if self.has_explicit_mode and sys.platform != "win32":  # pragma: win32 no cover
            with suppress(PermissionError):
                os.fchmod(fd, self._context.mode & ~_OTHERS_WRITE)

    def _try_break_stale_lock(self) -> None:
        with suppress(OSError, ValueError), pin_inode(self._lock_path) as pinned:
            content, marker = _read_lock_file(self._lock_path)
            if pinned not in {None, marker.st_ino}:  # pragma: win32 no cover  # Windows pins nothing
                return  # another node replaced the pinned one before the read, and nothing keeps its number from reuse
            if content == STRICT_SOFT_SENTINEL_RECORD:  # pragma: needs hard-link
                return
            # Such a marker is held for as long as its owner runs, so being unchanged proves nobody gone.
            if content == _RECORDLESS_MARKER:
                return
            holder = _parse_lock_holder(content)

            if holder is None:
                # Unparsable: wrong line count, a non-integer PID or start token, empty, oversized or not UTF-8.
                # Self-heal only once the file is clearly not a half-written fresh lock (a peer between O_EXCL and
                # _write_lock_info), so the brief create-then-write window is never mistaken for a stale lock.
                if self._marker_unchanged_for(marker) >= MALFORMED_LOCK_AGE_THRESHOLD:
                    self._take_over(marker, _placeholder_breaker(content))
                return

            if owner_is_stale(*holder):
                self._take_over(marker, holder[0])

    def _try_break_expired_lock(self) -> None:
        if (lifetime := self._context.lifetime) is None:
            return
        with suppress(OSError), pin_inode(self._lock_path) as pinned:
            # _read_lock_file takes the lstat, not stat: a symlink swapped in at the path must report its own mtime.
            content, marker = _read_lock_file(self._lock_path)
            if pinned not in {None, marker.st_ino} or time.time() - marker.st_mtime < lifetime:
                return
            holder = self._recorded_holder(content)
            self._take_over(marker, None if holder is None else holder[0])

    def _take_over(self, marker: os.stat_result, breaker_pid: int | None) -> None:
        # Swap the judged marker for a complete marker of ours in one exchange, so the lock path never stands empty.
        # Once the file swapped out proves to be the one judged, this lock holds the path; the caller's pin keeps that
        # file's inode number from naming anything else. Only a peer's exchange can move our marker after that, and
        # the peer puts it back because what it took is not the file it judged. Every failure before the handoff takes
        # our marker off the path and puts back what the exchange took. Without an exchange, the portable break decides.
        if exchange_supported():  # pragma: win32 no cover
            self._take_over_by_exchange(marker, breaker_pid)
        else:
            break_lock_file(self._lock_path, marker.st_mtime, marker.st_ino)

    def _take_over_by_exchange(self, marker: os.stat_result, breaker_pid: int | None) -> None:  # pragma: win32 no cover
        if (current := os.lstat(self._lock_path)).st_mtime != marker.st_mtime or current.st_ino != marker.st_ino:
            return
        if breaker_pid is not None and self._restore_stranded_marker(breaker_pid, marker):
            return
        aside: Final = f"{self._lock_path}.break.{os.getpid()}.{secrets.token_hex(16)}"
        flags: Final = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        fd: Final = os.open(aside, flags, self._open_mode() & ~_OTHERS_WRITE)
        self._mark_descriptor_pending(fd)
        exchanged = held = False
        try:
            identity: Final = _file_identity(os.fstat(fd))
            self._apply_explicit_mode(fd)
            self._write_lock_info(fd)
            # The scan and the write take long enough for a peer to have taken the marker since the check above, and an
            # exchange now would move that peer's live marker aside.
            if (current := os.lstat(self._lock_path)).st_mtime != marker.st_mtime or current.st_ino != marker.st_ino:
                return
            if not exchange_names(aside, self._lock_path):
                return  # NFS, SMB and the like refuse it; renaming the marker aside would empty the path
            exchanged = True
            if (taken := os.lstat(aside)).st_mtime != marker.st_mtime or taken.st_ino != marker.st_ino:
                return  # a peer recreated the marker, which the cleanup below swaps back
            Path(aside).unlink()
            exchanged = False
            self._mark_descriptor_owned(fd, identity)
            held = True
        finally:
            if not held:
                self._abandon_take_over(fd, aside, put_back=exchanged)

    def _restore_stranded_marker(self, breaker_pid: int, marker: os.stat_result) -> bool:  # pragma: win32 no cover
        # A breaker killed between its exchange and the put-back leaves its own marker at the path and the live
        # holder's marker under its break name. A recoverer first renames that record to a name of its own. Only one
        # contender wins the rename, so two of them never exchange the same record and reverse each other. A record
        # a dead recoverer holds is claimable again. A claim of this process that no thread is working on was
        # abandoned by a failed put-back, and the next poll takes it up again. Where the marker cannot be put back
        # the lock counts as held, as it does wherever no exchange is possible.
        if (candidates := self._stranded_candidates(breaker_pid)) is None:
            return True
        return any(self._recover_claimed(path, breaker_pid, marker, adopt=abandoned) for path, abandoned in candidates)

    def _stranded_candidates(self, breaker_pid: int) -> list[tuple[str, bool]] | None:  # pragma: win32 no cover
        directory, name = os.path.split(self._lock_path)
        stranded: Final = re.compile(rf"{re.escape(name)}\.break\.{breaker_pid}\.[0-9a-f]{{32}}")
        # [0-9], not \d: a str pattern's \d takes any Unicode digit, and int() reads fullwidth ones as another number.
        claimed: Final = re.compile(rf"{re.escape(name)}\.restore\.([0-9]+)\.[0-9a-f]{{32}}")
        candidates: list[tuple[str, bool]] = []
        with os.scandir(directory or os.curdir) as entries:
            for entry in entries:
                if not entry.is_file(follow_symlinks=False):
                    continue
                path = str(Path(directory) / entry.name)
                if stranded.fullmatch(entry.name):
                    candidates.append((path, False))
                elif (found := claimed.fullmatch(entry.name)) and 1 <= (recoverer := int(found[1])) <= 2**31 - 1:
                    # No filelock writes a claim outside the pid range; os.kill would raise OverflowError on one.
                    ours = recoverer == os.getpid()
                    if not (ours or owner_is_stale(recoverer, host_name(), None)):
                        return None
                    candidates.append((path, ours))
        return sorted(candidates)

    def _recover_claimed(
        self, path: str, breaker_pid: int, marker: os.stat_result, *, adopt: bool
    ) -> bool:  # pragma: win32 no cover
        # One try/finally owns the registration from the first step to the last, so no interruption can leave it set.
        # Registered before the rename publishes the name, so a thread that scans meanwhile sees a claim in progress.
        own: Final = path if adopt else f"{self._lock_path}.restore.{os.getpid()}.{secrets.token_hex(16)}"
        if not _begin_claim(own):
            return True
        try:
            if not adopt:
                try:
                    Path(path).rename(own)
                except OSError:  # a peer that won the rename now holds the record, so the lock counts as held
                    return True
            return self._restore_claimed(own, breaker_pid, marker)
        finally:
            _end_claim(own)

    def _restore_claimed(self, own: str, breaker_pid: int, marker: os.stat_result) -> bool:  # pragma: win32 no cover
        try:
            content, _ = _read_lock_file(own)
        except OSError:
            self._return_claim(own, breaker_pid)
            return True
        if not self._marker_is_live(content, breaker_pid):
            with suppress(OSError):
                Path(own).unlink()
            return False
        # The inode of the marker judged dead must still be at the path, or a peer has acted since.
        with suppress(OSError):
            current = os.lstat(self._lock_path)
            if (current.st_ino, current.st_mtime) == (marker.st_ino, marker.st_mtime) and exchange_names(
                own, self._lock_path
            ):
                if os.lstat(own).st_ino == marker.st_ino:
                    Path(own).unlink()
                    return True
                # A peer replaced the path after the check, so the exchange took its marker: swap it straight back.
                exchange_names(own, self._lock_path)
        self._return_claim(own, breaker_pid)
        return True

    def _return_claim(self, own: str, breaker_pid: int) -> None:  # pragma: win32 no cover
        # A refused rename leaves the claim under this process's PID, and the next poll retries it.
        with suppress(OSError):
            Path(own).rename(f"{self._lock_path}.break.{breaker_pid}.{secrets.token_hex(16)}")

    @classmethod
    def _marker_is_live(cls, content: str | None, breaker_pid: int) -> bool:  # pragma: win32 no cover
        if content == STRICT_SOFT_SENTINEL_RECORD:  # pragma: needs hard-link
            return True
        if content == _RECORDLESS_MARKER:
            # An empty marker is a pre-3.22 holder for as long as it exists, as the stale check treats it.
            return True
        # The breaker's own marker names its PID on this host; the same number on another host is a holder.
        return (
            (holder := _parse_lock_holder(content)) is not None
            and (holder[0] != breaker_pid or holder[1] != host_name())
            and not owner_is_stale(*holder)
        )

    def _abandon_take_over(self, fd: int, aside: str, *, put_back: bool) -> None:  # pragma: win32 no cover
        self._mark_descriptor_released()
        put_back_done = not put_back
        try:
            if put_back:
                put_back_done = exchange_back(self._lock_path, aside, fd)
        finally:
            try:
                # Whichever name still holds our marker loses it: the descriptor keeps its inode number ours.
                own: Final = os.fstat(fd)
                for name in (self._lock_path, aside):
                    if name == self._lock_path and not put_back_done:
                        continue
                    with suppress(FileNotFoundError):
                        if os.path.samestat(os.lstat(name), own):
                            Path(name).unlink()
            finally:
                os.close(fd)

    def _marker_unchanged_for(self, marker: os.stat_result) -> float:
        # The mtime is the writer's or file server's clock, so against time.time() a contender whose clock runs ahead
        # would expire a fresh marker. Only watch it for change, timed on this process's monotonic clock. The owner of a
        # file sets its mtime, and ext4 reuses a freed inode number, so a new marker can repeat both. The change time
        # cannot be set and moves with every create, which keeps a new marker from inheriting an old one's age.
        now: Final = time.monotonic()
        key: Final = marker.st_mtime, marker.st_ino, marker.st_ctime_ns
        if (seen := self._marker_seen) is None or seen[0] != key:
            self._marker_seen = key, now
            return 0.0
        return now - seen[1]

    def _unprobeable_holder(self) -> str | None:
        # This host cannot prove such a holder dead, so whoever decides whether it is gone needs its name.
        with suppress(OSError, ValueError):
            if (content := _read_lock_file(self._lock_path)[0]) == _RECORDLESS_MARKER:
                return "an owner that recorded no PID (a filelock release before 3.22, or one still writing its record)"
            if (holder := self._recorded_holder(content)) and holder[1] != host_name():
                return f"pid {holder[0]} on {holder[1]}"
        return None

    @classmethod
    def _recorded_holder(cls, content: str | None) -> tuple[int, str] | None:
        return None if (holder := _parse_lock_holder(content)) is None else holder[:2]

    @staticmethod
    def _write_lock_info(fd: int) -> None:
        # No suppression: a write failure must reach the acquisition rollback so it never publishes a half-written
        # marker as held state. The optional third line is this process's start token, absent when the platform
        # exposes no proven start time, in which case a reader falls back to PID-only liveness.
        info = f"{os.getpid()}\n{host_name()}\n"
        if (token := process_start_token(os.getpid())) is not None:
            info += f"{token}\n"
        write_all(fd, info.encode())

    @property
    def pid(self) -> int | None:
        """
        The PID of the process holding this lock, read from the lock file.

        :returns: the PID as an integer, or ``None`` if the lock file does not exist or cannot be parsed

        """
        with suppress(OSError, ValueError):
            holder = _parse_lock_holder(_read_lock_file(self._lock_path)[0])
            if holder is not None:
                return holder[0]
        return None

    @property
    def is_lock_held_by_us(self) -> bool:
        """
        Whether this lock is held by the current process.

        :returns: ``True`` if the lock file exists and names the current process's PID, hostname and, when recorded,
            start token

        """
        with suppress(OSError, ValueError):
            if (holder := _parse_lock_holder(_read_lock_file(self._lock_path)[0])) is not None:
                return owner_is_current_process(*holder)
        return False

    def break_lock(self) -> None:
        """Forcibly break the lock by removing the lock file, regardless of who holds it."""
        with suppress(OSError):
            Path(self._lock_path).unlink()

    def _release_current(self, *, force: bool) -> None:
        try:
            super()._release_current(force=force)
        finally:
            # A release a handler cut short after the descriptor closed still owes the unlink on Windows.
            self._finish_marker_cleanup()

    def _release(self) -> None:
        fd = self._context.lock_file_fd
        assert fd is not None  # ruff:ignore[assert]  # _release runs only while held, so the descriptor is set
        # Capture the held file's identity before closing so cleanup can refuse to unlink a successor's marker. A
        # supported lifetime lease lets a peer break our expired marker and create its own at this path before we
        # release; unlinking by path alone would then delete the successor's lock.
        identity: tuple[int, int] | None = None
        with suppress(OSError):
            identity = _file_identity(os.fstat(fd))
        if sys.platform == "win32":  # pragma: win32 cover
            self._release_then_unlink(fd, identity)
        else:  # pragma: win32 no cover
            # Our open descriptor keeps the inode allocated, so its number cannot name a successor's marker yet, and an
            # interrupt before the unlink lands leaves the lock held for the retried release to finish.
            unlinked: Final = self._unlink_open_marker(fd, identity)
            # A failed close may already have released and recycled the descriptor number. Relinquish it before the
            # one close attempt so no later release can close an unrelated descriptor that reused the same integer.
            self._mark_descriptor_released()
            try:
                self._close_released_fd(fd, default_suppresses=False)
            finally:
                # A share that refuses to delete an open file gets the unlink after the close; the marker still exists
                # until then, so its inode cannot be reused either.
                if not unlinked:
                    self._unlink_held_marker(identity)

    def _release_then_unlink(self, fd: int, identity: tuple[int, int] | None) -> None:  # pragma: win32 cover
        # Windows cannot unlink an open file, so the unlink outlives the hold until it lands, here or on a later acquire
        # or release. Only the gate's holder touches the pending record, so two threads never overwrite each other's.
        with self._transition_lock:
            self._marker_fds = [fd]
            self._marker_cleanup = identity
            self._mark_descriptor_released()
            try:
                self._close_marker_fd()
            # Marker cleanup must also run for control-flow exceptions, and both failures must remain observable.
            except BaseException as close_error:
                try:
                    self._finish_marker_cleanup()
                except BaseException as cleanup_error:  # ruff:ignore[blind-except]  # keep control-flow failures
                    _raise_grouped_errors(
                        "lock descriptor close and marker cleanup both failed",
                        close_error,
                        cleanup_error,
                    )
                raise
            self._finish_marker_cleanup()

    def _marker_names(self) -> tuple[str, ...]:
        # After a rename of the lock's directory only the caller's spelling, resolved through the working directory,
        # still reaches the marker this hold opened.
        return tuple(dict.fromkeys((self._lock_path, self.lock_file)))

    def _unlink_open_marker(self, fd: int, identity: tuple[int, int] | None) -> bool:  # pragma: win32 no cover
        if identity is None:
            return True
        replaced = False
        for name in self._marker_names():
            try:
                if _file_identity(os.lstat(name)) == identity:
                    Path(name).unlink()
                    return True
                replaced = True
            except FileNotFoundError:
                continue
            except OSError:
                return False
        if replaced:
            try:
                _leave_released_record(fd)
            except OSError:
                return False
        return True

    def _finish_marker_cleanup(self) -> None:
        # Never waits: whoever holds the gate is acquiring or releasing this lock and finishes the cleanup itself.
        if self._marker_cleanup is not None and self._transition_lock.acquire(blocking=False):  # pragma: win32 cover
            try:
                # A close error here belongs to the release an interrupt cut short, not to this caller.
                with suppress(OSError):
                    self._close_marker_fd()
                if (identity := self._marker_cleanup) is not None:  # pragma: no branch  # the gate's holder clears it
                    self._windows_unlink_if_ours(identity)
            finally:
                self._transition_lock.release()

    def _close_marker_fd(self) -> None:  # pragma: win32 cover
        # Popping and closing is one chain of C calls that no signal handler can split, so a retried release closes the
        # descriptor at most once; a second close could hit an unrelated file that reused the number.
        try:
            list(map(os.close, islice(iter(vars(self).setdefault("_marker_fds", []).pop, None), 1)))
        except IndexError:
            return
        except OSError:
            if not self._suppresses_close_error(default_suppresses=False):
                raise

    def _unlink_held_marker(self, identity: tuple[int, int] | None) -> None:
        if sys.platform == "win32":  # pragma: win32 cover
            if identity is not None:
                self._windows_unlink_if_ours(identity)
        else:  # pragma: win32 no cover
            for name in self._marker_names():
                with suppress(OSError):
                    if _file_identity(os.lstat(name)) == identity:
                        Path(name).unlink()
                        return

    def _windows_unlink_if_ours(self, identity: tuple[int, int]) -> None:  # pragma: win32 cover
        retry_delay = 0.001
        for attempt in range(_UNLINK_MAX_RETRIES):
            # Windows doesn't immediately release file handles after close, causing EACCES/EPERM on unlink. Recheck
            # identity each attempt: a failed unlink leaves a window for a successor to replace the marker at this path.
            try:
                for name in self._marker_names():
                    with suppress(FileNotFoundError):
                        if _file_identity(os.lstat(name)) == identity:
                            # Forgotten before the unlink: Windows does not promise unique file IDs, so no retry may
                            # reach a successor that reused ours.
                            self._marker_cleanup = None
                            unlink_path(name)
                            break
            except OSError as exc:
                if exc.errno in {EACCES, EPERM}:
                    # The file is still ours, so the next acquire or release tries again if these attempts run out.
                    self._marker_cleanup = identity
                    if attempt < _UNLINK_MAX_RETRIES - 1:
                        time.sleep(retry_delay)
                        retry_delay *= 2
                    continue
            self._marker_cleanup = None
            return


def _file_identity(st: os.stat_result) -> tuple[int, int]:
    # (st_dev, st_ino) names the concrete inode behind a path, so a marker recreated at the same pathname after an
    # expired lease reads as a different file. CPython populates both on Windows from the volume serial and file index.
    return st.st_dev, st.st_ino


def _begin_claim(name: str | Path) -> bool:  # pragma: win32 no cover
    # Keyed by the file name alone: its random token identifies the claim, and the spelling of the directory varies.
    with _CLAIMS_LOCK:
        if (key := Path(name).name) in _CLAIMS_IN_PROGRESS:
            return False
        _CLAIMS_IN_PROGRESS.add(key)
        return True


def _end_claim(name: str | Path) -> None:  # pragma: win32 no cover
    with _CLAIMS_LOCK:
        _CLAIMS_IN_PROGRESS.discard(Path(name).name)


def _placeholder_breaker(content: str | None) -> int | None:  # pragma: win32 no cover
    # The placeholder a break leaves at the path while it works names the breaker that may have died there.
    if content is None or not content.startswith(placeholder := "filelock/break\n") or not content.endswith("\n"):
        return None
    with suppress(ValueError):
        return parse_decimal(content.removeprefix(placeholder)[:-1])
    return None


def _leave_released_record(fd: int) -> None:
    # A peer's exchange holds our marker under its break name and will put it back. Overwriting the record lets
    # contenders age it out instead of waiting on this live PID. The write comes before the truncate so the file is
    # never empty.
    if sys.platform != "win32":  # pragma: win32 no cover
        os.pwrite(fd, released := b"filelock/released\n", 0)
        os.ftruncate(fd, len(released))


def _read_lock_file(path: str) -> tuple[str | None, os.stat_result]:
    # A legitimate lock file is always a regular file. Classify the path with lstat first, so any other node (symlink,
    # FIFO, socket, device) is reported as a malformed lock the caller can evict, without an os.open that would follow
    # a symlink, stall on a FIFO, or fail on a socket and leave acquisition wedged. The stat result still flows back
    # for the identity-checked stale break. lstat, not stat, so a hostile symlink is never followed onto its target.
    st = os.lstat(path)
    if not stat.S_ISREG(st.st_mode):  # pragma: needs fifo
        return None, st
    # Re-check on the opened handle: O_NOFOLLOW refuses a symlink swapped in after the lstat, O_NONBLOCK stops a FIFO
    # swapped in from stalling the open, and the fstat catches any other non-regular replacement race before we read.
    # The capped read stops a huge regular file (e.g. one filled from /dev/zero) from exhausting memory.
    if sys.platform == "win32":  # pragma: win32 cover
        # os.open would wait for an oplock holder to answer its break, past the caller's deadline.
        fd = open_read_without_oplock_wait(path)
    else:  # pragma: win32 no cover
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):  # pragma: no cover  # only a non-regular node swapped in after the lstat
            return None, st
        data = os.read(fd, _MAX_LOCK_FILE_SIZE + 1)
    finally:
        os.close(fd)
    if len(data) <= _MAX_LOCK_FILE_SIZE:
        with suppress(UnicodeDecodeError):
            return data.decode("utf-8"), st
    return None, st


def _parse_lock_holder(content: str | None) -> tuple[int, str, int | None] | None:
    # A well-formed lock file is "<pid>\n<hostname>\n" with an optional "<start_token>\n" third line naming the
    # holder's process start instant (a filelock 3.29 marker wrote this only on Windows; every platform writes it now).
    # Anything else (wrong line count, a non-integer PID or start token, empty or unreadable content) is unparsable;
    # returning None lets the caller treat it as a malformed lock to self-heal rather than a holder.
    # Every writer since 3.22 ends each line with "\n", so a record without one was cut by a crash or read mid-write. As
    # a holder it would name a foreign host no one reclaims, or a start token that lets a contender break a live lock.
    if not content or not content.endswith("\n") or len(lines := content.strip().splitlines()) not in {2, 3}:
        return None
    try:
        pid = parse_decimal(lines[0])
        start_token = parse_decimal(lines[2]) if len(lines) == _MARKER_WITH_START_TOKEN_LINE_COUNT else None
    except ValueError:
        return None
    # A pid outside the valid range is a malformed lock, not a holder. Without this, a non-positive pid
    # reaches os.kill() where 0 / -1 mean "the caller's own process group / every process" so a dead
    # holder reads as alive and the lock is never reclaimed, while an oversized pid raises OverflowError
    # (not OSError/ValueError) out of the self-heal path. _parse_marker_bytes already enforces this range.
    if not 1 <= pid <= 2**31 - 1:
        return None
    return pid, lines[1], start_token


def parse_decimal(text: str) -> int:
    """
    Read a PID or start token as a writer formats it: ASCII digits only.

    :raises ValueError: for anything else ``int()`` would accept, such as ``1_000``, surrounding whitespace, a sign or
        non-ASCII digits; no filelock writes those

    """
    if not (text.isascii() and text.isdigit()):
        msg = f"not a decimal integer: {text!r}"
        raise ValueError(msg)
    return int(text)


__all__ = [
    "MALFORMED_LOCK_AGE_THRESHOLD",
    "SoftFileLock",
    "parse_decimal",
]
