from __future__ import annotations

__lazy_modules__ = [
    "contextlib",
    "filelock._error",
    "filelock._identity",
    "filelock._marker",
    "filelock._soft",
    "filelock._util",
    "functools",
    "pathlib",
    "secrets",
    "tempfile",
]

import contextlib
import errno
import functools
import os
import secrets
import stat
import sys
import tempfile
import time
from dataclasses import dataclass
from errno import EACCES, EEXIST, ENOENT, ENOSYS, EPERM, ESTALE, EXDEV
from pathlib import Path
from typing import TYPE_CHECKING, Final, Literal, cast

from filelock._api import BaseFileLock, ThreadLocalFileContext, _canonical, _raise_cleanup_errors
from filelock._error import SoftFileLockProtocolError
from filelock._identity import host_name, owner_is_stale, process_start_token
from filelock._marker import parse_marker
from filelock._soft import MALFORMED_LOCK_AGE_THRESHOLD, _parse_lock_holder
from filelock._soft_protocol import STRICT_SOFT_SENTINEL_RECORD
from filelock._util import foreign_directory_owner, write_all

if sys.platform == "win32":  # pragma: win32 cover
    from filelock._windows import _is_unlinked, _open_deletable_for_reading, _pin_directory, _unpin_directory

if TYPE_CHECKING:
    from collections.abc import Callable, Iterator

StrictSoftFileClaimState = Literal["intent", "held"]
_OccupantState = Literal["sentinel", "held", "exited", "damaged", "unparsable"]

_CLAIM_STATES: Final[frozenset[str]] = frozenset({"intent", "held"})
_COORDINATION_SUFFIX: Final[str] = ".filelock"
_CLAIM_DIRECTORY_NAME: Final[str] = "claims"
_CLAIM_MAGIC: Final[str] = "filelock-strict-v1"
_CLAIM_RECORD_LIMIT: Final[int] = 1024
_CLAIM_NAME_PART_COUNT: Final[int] = 3
_TOKEN_HEX_LENGTH: Final[int] = 32
_PRIVATE_RECORD_MARKER: Final[str] = ".private-v1-"
_PRIVATE_RECORD_SUFFIX: Final[str] = ".tmp"
_PRIVATE_RECORD_RANDOM_HEX_LENGTH: Final[int] = 32
_CLAIM_PROBE_PREFIX: Final[str] = ".probe-v1-"
_PRIVATE_RECORD_GRACE: Final[float] = 2.0
_UNLINK_MAX_RETRIES: Final[int] = 10
#: How long a scan waits out a claim held in Windows' delete-pending state before treating it as unreadable.
_CLAIM_READ_GRACE: Final[float] = 0.5
_CLAIM_READ_RETRY: Final[float] = 0.002
#: Rides out a peer replacing the lock path mid-inspection; a foreign node or unreadable record outlasting it is damage.
_SENTINEL_READ_GRACE: Final[float] = 0.5
_NODE_DESCRIPTIONS: Final[dict[int, str]] = {stat.S_IFDIR: "a directory", stat.S_IFLNK: "a symlink"}
#: Windows opens descriptors in text mode by default, which rewrites newlines and truncates a record at a control byte.
#: The claim and sentinel records are exact binary, so every record descriptor must be binary; POSIX ignores the flag.
_O_BINARY: Final[int] = getattr(os, "O_BINARY", 0)
_LEGACY_SENTINEL: Final[bytes] = STRICT_SOFT_SENTINEL_RECORD.encode()
_WINDOWS_HARD_LINK_UNSUPPORTED: Final[frozenset[int]] = frozenset({1, 17, 50})

# Termux/Android CPython ships without os.link (bionic long had only linkat), so the strict backend's whole hard-link
# mechanism is absent there. Probe once and gate every os.link reference on it, so importing filelock still works and
# only an actual StrictSoftFileLock acquire reports the missing capability.
_HAS_LINK: Final[bool] = hasattr(os, "link")

# Probe dir_fd capability once at import. A per-call ``os.unlink in os.supports_dir_fd`` check flips to False the moment
# a test mocks os.unlink, silently diverting the code to a different branch than the one under test.
_OPEN_SUPPORTS_DIR_FD: Final[bool] = os.open in os.supports_dir_fd
_UNLINK_SUPPORTS_DIR_FD: Final[bool] = os.unlink in os.supports_dir_fd
_STAT_SUPPORTS_DIR_FD: Final[bool] = os.stat in os.supports_dir_fd
_LINK_SUPPORTS_DIR_FD: Final[bool] = _HAS_LINK and os.link in os.supports_dir_fd
_MKDIR_SUPPORTS_DIR_FD: Final[bool] = os.mkdir in os.supports_dir_fd
_SCANDIR_SUPPORTS_FD: Final[bool] = os.scandir in os.supports_fd


def _probe_link_follow_symlinks() -> bool:
    # os.supports_follow_symlinks lists os.link on PyPy, but its linkat then rejects follow_symlinks=False with EINVAL,
    # and Windows raises NotImplementedError for the option outright. Link a throwaway file for real so the answer
    # reflects the runtime rather than its advertisement, and treat any failure as "not honored": the option only
    # hardens a source this process created with O_EXCL, so skipping it is safe, and a real environment fault surfaces
    # when the actual link runs.
    if not _HAS_LINK:
        return False
    try:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory, "probe-source")
            source.touch()
            os.link(source, Path(directory, "probe-link"), follow_symlinks=False)
    except (OSError, NotImplementedError, ValueError):
        return False
    return True


_LINK_HONORS_FOLLOW_SYMLINKS: Final[bool] = _probe_link_follow_symlinks()


def _probe_hard_link_unsupported_errnos() -> frozenset[int]:
    # GraalPy's errno omits ENOTSUP, so importing the name outright breaks every runtime that ships without it. ENOTSUP
    # wins wherever it exists, leaving every runtime that names it unchanged. EOPNOTSUPP only stands in for the ones
    # that do not, and it approximates rather than matches: the two codes agree on Linux but differ on macOS/BSD and on
    # Windows. Where neither exists a runtime that cannot name "operation not supported" cannot raise it either, and
    # ENOSYS/EXDEV still classify the link failures it can raise.
    not_supported = getattr(errno, "ENOTSUP", getattr(errno, "EOPNOTSUPP", None))
    return frozenset({ENOSYS, EXDEV} if not_supported is None else {ENOSYS, EXDEV, not_supported})


_HARD_LINK_UNSUPPORTED_ERRNOS: Final[frozenset[int]] = _probe_hard_link_unsupported_errnos()


class StrictSoftFileLock(BaseFileLock):
    """Portable fail-closed lock based on immutable owner claims."""

    _preserve_lock_file_supported: bool = True
    _on_acquired_supported: bool = False
    #: Age cannot clear a strict claim: expiring one on a clock is the overlap the fail-closed contract exists to rule
    #: out, so only force_break() removes it.
    _lifetime_supported: bool = False
    _lifetime_unsupported_reason: str = "a strict claim is never broken by age, only by force_break()"
    #: Contending processes each publish and rescan several files, so jitter their retries. Seconds. Only a
    #: poll_interval below this cap jitters; the default equals it and keeps a fixed cadence.
    _poll_backoff_cap: float = 0.05
    _occupant_seen: tuple[tuple[float, int, int], float] | None = None
    #: Set by the finalizer, after which no retry of a failed cleanup can run.
    _finalizing: bool = False

    def _acquire(self) -> None:
        # Finish a doorway whose claim cleanup failed before publishing, so this owner never contends with leftovers.
        self._discard_pending_doorways(exited_only=True)
        lock_path = Path(self._lock_path)
        coordination_directory = Path(f"{lock_path}{_COORDINATION_SUFFIX}")
        claim_directory = coordination_directory / _CLAIM_DIRECTORY_NAME
        # A process that can write these directories can remove claims from them, so they take the lock's mode plus
        # search permission where the mode grants read.
        directory_mode: Final = (mode := self._open_mode()) | (mode & 0o444) >> 2
        _ensure_protocol_directory(self.lock_file, coordination_directory, directory_mode)
        _ensure_claim_directory(self.lock_file, claim_directory, directory_mode)
        if (
            sentinel_fd := _open_or_create_sentinel(
                self.lock_file, lock_path, self._open_mode(), self._occupant_unchanged_for
            )
        ) is None:
            return
        try:
            sentinel_identity = _file_identity(os.fstat(sentinel_fd))
        except BaseException as inspection_error:  # preserve inspection and descriptor cleanup errors
            try:
                os.close(sentinel_fd)
            except BaseException as close_error:  # ruff:ignore[blind-except]  # preserve inspection and descriptor cleanup errors
                _raise_cleanup_errors("strict sentinel inspection cleanup failed", inspection_error, close_error)
            raise
        self._mark_descriptor_pending(sentinel_fd, sentinel_identity)
        try:
            if won := self._attempt_doorway(claim_directory):
                self._mark_descriptor_owned(sentinel_fd, sentinel_identity)
        except BaseException:
            # Once recorded as held, the acquire's rollback releases the claim instead.
            if not self.is_locked:
                self._discard_doorway()
            raise
        if not won:
            self._discard_doorway()

    def _discard_pending_doorways(self, *, exited_only: bool) -> None:
        if self._context.owner_claim_paths and not self.is_locked:
            self._discard_doorway()
        if not isinstance(context := self._context, ThreadLocalFileContext):
            return
        # Only this registry reaches an exited thread's thread-local doorway; a live thread finishes its own.
        own: Final = context.current
        with self._transition_lock:
            for pending, thread in self._thread_pending.copy().values():
                if exited_only and thread.is_alive():
                    continue
                context.current = pending
                try:
                    self._discard_doorway()
                finally:
                    context.current = own

    def _occupant_unchanged_for(self, occupant: os.stat_result) -> float:
        # SoftFileLock's marker rule: the mtime is only tested for change, timed on this process's monotonic clock.
        key, now = (occupant.st_mtime, occupant.st_ino, occupant.st_size), time.monotonic()
        if (seen := self._occupant_seen) is None or seen[0] != key:
            self._occupant_seen = key, now
            return 0.0
        return now - seen[1]

    def _attempt_doorway(self, claim_directory: Path) -> bool:
        if _read_existing_claims(self.lock_file, claim_directory):
            return False

        token = secrets.token_hex(_TOKEN_HEX_LENGTH // 2)
        intent_name = _claim_name("intent", token)
        intent_path = str(claim_directory / intent_name)
        # Record each claim before its link: once the link may have committed, any later failure must leave the name for
        # _discard_doorway. The token makes the name unique, so removing it when the link never happened is a no-op.
        self._context.owner_claim_paths = (intent_path,)
        try:
            with _pin_claim_directory(claim_directory):
                publication_cleanup_error = _publish_record(
                    intent_path, _claim_record(token), self._open_mode(), _open_claim_directory
                )
        except _PrivateRecordReclaimedError:
            return False
        except (NotImplementedError, OSError) as error:
            _raise_if_hard_links_unsupported(self.lock_file, error)
            if isinstance(error, OSError) and error.errno == EEXIST:
                # Another file holds the name, so it is not this owner's to remove.
                self._context.owner_claim_paths = ()
                return False
            raise
        if publication_cleanup_error is not None:  # pragma: needs dir-fd
            raise publication_cleanup_error

        claims = _read_existing_claims(self.lock_file, claim_directory)
        if (
            not claims
            or any(claim.state == "held" for claim in claims)
            or min(claim.name for claim in claims) != intent_name
        ):
            return False

        held_name = _claim_name("held", token)
        held_path = str(claim_directory / held_name)
        self._context.owner_claim_paths = (held_path, intent_path)
        try:
            link_cleanup_error = _link_no_replace(claim_directory, intent_name, held_name)
        except (NotImplementedError, OSError) as error:
            _raise_if_hard_links_unsupported(self.lock_file, error)
            if isinstance(error, OSError) and error.errno == EEXIST:
                self._context.owner_claim_paths = (intent_path,)
            raise
        if link_cleanup_error is not None:  # pragma: needs dir-fd
            raise link_cleanup_error

        claims = _read_existing_claims(self.lock_file, claim_directory)
        # Keep the intent claim for the whole hold rather than unlinking it now. The intent has existed, unchanged,
        # since this owner published it, so a contender's os.scandir is guaranteed to return it (POSIX only leaves the
        # visibility of entries created or removed *during* a scan unspecified). The freshly linked held claim carries
        # no such guarantee: a scan that races its creation can miss it. Were the intent removed here, that scan could
        # observe neither claim and let a larger-token contender win over this owner. The stable intent is the witness
        # that keeps the phase-five min-token decision computed over the true set. Release unlinks both.
        return {intent_name, held_name}.issubset(claim.name for claim in claims) and min(
            _claim_token_key(claim.name) for claim in claims
        ) == f"v1-{token}.claim"

    @property
    def claims(self) -> tuple[StrictSoftFileClaim, ...]:
        """Published claims that block acquisition."""
        # An inspection writes nothing, so it trusts no record's age rather than probe the filesystem clock.
        return _read_existing_claims(self.lock_file, self._claim_directory, probe_clock=False, verify_server=False)

    @property
    def lock_path_occupant(self) -> str | None:
        """
        What sits at the lock path in place of the strict sentinel, or ``None`` when the sentinel or nothing is there.

        A soft-lock owner's marker blocks acquisition without appearing in :attr:`~filelock.StrictSoftFileLock.claims`;
        this describes it, naming the PID and host when the marker records them. A directory, a symlink, a marker whose
        owner on this host has exited, or a file it cannot parse that stays unchanged for two seconds makes acquisition
        raise :class:`~filelock.SoftFileLockProtocolError` instead and needs an operator to remove it.
        """
        try:
            fd, _, occupant = _inspect_lock_path(Path(_canonical(self.lock_file)))
        except FileNotFoundError:
            return None
        if fd is None:
            return occupant
        os.close(fd)
        return None

    def force_break(self, claim_name: str) -> None:
        """Remove one named claim, allowing overlap if its owner still holds the protected resource."""
        _validate_force_break_name(claim_name)
        with _pin_claim_directory(directory := self._claim_directory):
            _require_exact_name(directory, claim_name)
            cleanup_error = _unlink_in_directory(directory, claim_name)
        if cleanup_error is not None:  # pragma: needs dir-fd
            raise cleanup_error

    def _release_abandoned_holds(self) -> None:
        self._finalizing = True
        super()._release_abandoned_holds()
        # release() skips a doorway whose cleanup failed, as it is no hold, so a dropped instance would strand claims.
        with contextlib.suppress(Exception):
            # GraalPy defers finalizers to the host collector, so its tests cannot drive this cleanup.
            self._discard_pending_doorways(exited_only=False)  # pragma: needs collected-finalization

    def _reconcile_failed_acquire(self, canonical: str) -> None:
        # The acquisition is over, so the next one resolves the working directory again rather than reuse this one's.
        self._context.claim_root = None
        super()._reconcile_failed_acquire(canonical)

    def _release(self) -> None:
        fd = cast("int", self._context.lock_file_fd)
        self._context.claim_root = None
        remaining, errors = self._unlink_owner_claims()
        if remaining:
            _raise_recorded_errors("strict claim release failed", errors)
        self._mark_descriptor_released()
        try:
            self._close_released_fd(fd, default_suppresses=False)
        except BaseException as close_error:  # ruff:ignore[blind-except]  # preserve claim and sentinel cleanup errors
            errors.append(close_error)
        if errors:
            _raise_recorded_errors("strict release cleanup failed", errors)

    def _discard_doorway(self) -> None:
        remaining, errors = self._unlink_owner_claims()
        # The pending descriptor holds no lock and a retry needs the claim paths alone. A dropped instance does not
        # retry, so close it now.
        if (fd := self._context.pending_lock_file_fd) is not None:
            self._context.pending_lock_file_fd = self._context.pending_lock_file_fd_identity = None
            try:
                self._close_released_fd(fd, default_suppresses=False)
            except BaseException as close_error:  # ruff:ignore[blind-except]  # preserve claim and sentinel cleanup errors
                errors.append(close_error)
        if remaining:
            # Stay pending, not owned: is_locked reports only a held claim that passed the final rescan. The next
            # acquire retries this cleanup.
            _raise_recorded_errors("strict doorway claim cleanup failed", errors)
        self._mark_descriptor_released()
        if errors:
            _raise_recorded_errors("strict doorway cleanup failed", errors)

    def _unlink_owner_claims(self) -> tuple[list[str], list[BaseException]]:
        remaining, errors = _unlink_owner_paths(self._context.owner_claim_paths)
        self._context.owner_claim_paths = tuple(remaining)
        if self._finalizing:
            # No retry runs after the finalizer and no caller sees its errors, so report success and let it close the
            # descriptor even when claims remain.
            return [], []  # pragma: needs collected-finalization
        return remaining, errors

    @property
    def _claim_directory(self) -> Path:
        if self._context.owner_claim_paths:
            return Path(self._context.owner_claim_paths[0]).parent
        return Path(f"{_canonical(self.lock_file)}{_COORDINATION_SUFFIX}") / _CLAIM_DIRECTORY_NAME


@dataclass(frozen=True)
class StrictSoftFileClaim:
    """One parsed strict soft-lock claim."""

    name: str
    state: StrictSoftFileClaimState
    token: str
    pid: int
    hostname: str
    #: The owner's process start token, or ``None`` when the platform exposes no proven start time. A strict lock never
    #: reclaims a claim on its own, so this identifies the owner for tooling rather than driving any automatic break.
    start: int | None = None


class _PrivateRecordReclaimedError(Exception):
    pass


def _open_or_create_sentinel(
    lock_file: str, path: Path, mode: int, unchanged_for: Callable[[os.stat_result], float]
) -> int | None:
    try:
        return _open_sentinel(lock_file, path, unchanged_for)
    except FileNotFoundError:
        pass

    _reclaim_sentinel_private_records(lock_file, path)
    try:
        publication_cleanup_error = _publish_record(os.fspath(path), _LEGACY_SENTINEL, mode)
    except _PrivateRecordReclaimedError:
        return None
    except (NotImplementedError, OSError) as error:
        _raise_if_hard_links_unsupported(lock_file, error)
        if not isinstance(error, OSError) or error.errno != EEXIST:
            raise
    else:
        if publication_cleanup_error is not None:  # pragma: needs dir-fd
            raise publication_cleanup_error
    try:
        return _open_sentinel(lock_file, path, unchanged_for)
    except FileNotFoundError:
        return None


def _open_sentinel(lock_file: str, path: Path, unchanged_for: Callable[[os.stat_result], float]) -> int | None:
    # A live soft-lock owner blocks like any holder. Anything else raises: claims cannot show it and force_break()
    # cannot remove it, so waiting would wedge every strict contender with no diagnostic. A file no lock parses may be a
    # soft lock between its create and its write, so it raises only once unchanged as long as a soft contender waits.
    deadline: float | None = None
    while True:
        fd, state, occupant = _inspect_lock_path(path)
        if fd is not None or state == "held":
            return fd
        reason = f"expected the strict sentinel at the lock path, found {occupant}"
        if state == "unparsable":
            if unchanged_for(path.lstat()) < MALFORMED_LOCK_AGE_THRESHOLD:
                return None
            raise SoftFileLockProtocolError(
                lock_file, None, f"{reason}, unchanged for {MALFORMED_LOCK_AGE_THRESHOLD:g} seconds"
            )
        if deadline is None:
            deadline = time.monotonic() + _SENTINEL_READ_GRACE
        if state == "exited" or time.monotonic() >= deadline:
            raise SoftFileLockProtocolError(lock_file, None, reason)
        time.sleep(_CLAIM_READ_RETRY)


def _inspect_lock_path(path: Path) -> tuple[int | None, _OccupantState, str]:
    # "held" may be a soft-lock owner that still runs, "exited" is one this host proves gone, "unparsable" is content no
    # lock parses, and "damaged" is a node no lock writes.
    if not stat.S_ISREG(mode := path.lstat().st_mode):
        return None, "damaged", _NODE_DESCRIPTIONS.get(stat.S_IFMT(mode), "a special file")
    try:
        fd, record = _open_record((os.fspath(path.parent), None), path.name, _CLAIM_RECORD_LIMIT)
    except FileNotFoundError:
        raise
    except PermissionError as error:
        # Windows denies the read while a legacy holder keeps its marker open, and a holder running as another user
        # may leave it unreadable here, so a marker that cannot be read may still be held.
        return None, "held", f"a file it cannot read ({error.strerror or error})"
    except OSError as error:
        return None, "damaged", f"a file it cannot read ({error.strerror or error})"
    if record == _LEGACY_SENTINEL:
        return fd, "sentinel", "the strict sentinel"
    os.close(fd)
    return None, *_describe_soft_marker(record.decode("utf-8", errors="replace"))


def _describe_soft_marker(content: str) -> tuple[_OccupantState, str]:
    # A SoftFileLock, MarkerSoftFileLock or SoftFileLease owner holds the path legitimately until it releases, so its
    # marker is contention. One this host proves gone is named, never removed: strict never acts on an inferred death.
    if (record := parse_marker(content)) is not None:
        pid, hostname, start = record.pid, record.hostname, record.start
    elif (holder := _parse_lock_holder(content)) is not None:
        pid, hostname, start = holder
    else:
        return "unparsable", "a file that is neither the sentinel nor a soft-lock marker it can parse"
    if owner_is_stale(pid, hostname, start):
        return "exited", f"a soft-lock marker left by pid {pid} on {hostname}, which has exited"
    return "held", f"a soft-lock marker held by pid {pid} on {hostname}"


def _read_claims(
    lock_file: str, directory_ref: tuple[str, int | None], *, probe_clock: bool, verify_server: bool
) -> tuple[StrictSoftFileClaim, ...]:
    names: Final = _list_claim_names(lock_file, directory_ref, probe_clock=probe_clock)
    name_parts: dict[str, tuple[StrictSoftFileClaimState, str]] = {}
    for name in names:
        if (parts := _parse_claim_name(name)) is None:
            raise SoftFileLockProtocolError(lock_file, name, "unknown claim name or protocol version")
        name_parts[name] = parts
    records: Final = _read_claim_records(lock_file, directory_ref, names, probe_clock=probe_clock)
    return tuple(
        _parse_claim(lock_file, name, name_parts[name], records[name])
        for name in names
        if name in records and (not verify_server or _claim_exists_on_server(lock_file, directory_ref, name))
    )


def _claim_exists_on_server(lock_file: str, directory_ref: tuple[str, int | None], name: str) -> bool:
    # Reading an NFS claim can succeed from a client cache after its server-side name is gone. LINK checks the source
    # on the server; a private proof is reclaimed if this process exits before unlinking it.
    proof = f"{_CLAIM_PROBE_PREFIX}{secrets.token_hex(_TOKEN_HEX_LENGTH // 2)}{_PRIVATE_RECORD_SUFFIX}"
    try:
        _link_or_find_committed(directory_ref, name, proof)
    except OSError as error:
        if error.errno in {ENOENT, ESTALE}:
            return False
        reason = f"cannot verify claim: {error.strerror or type(error).__name__}"
        raise SoftFileLockProtocolError(lock_file, name, reason) from error
    except NotImplementedError as error:
        raise SoftFileLockProtocolError(lock_file, name, "cannot verify claim: hard links unavailable") from error
    try:
        _unlink_relative(directory_ref, proof)
    except FileNotFoundError:
        pass
    except OSError as error:
        reason = f"cannot remove claim probe: {error.strerror or type(error).__name__}"
        raise SoftFileLockProtocolError(lock_file, name, reason) from error
    return True


def _list_claim_names(lock_file: str, directory_ref: tuple[str, int | None], *, probe_clock: bool) -> list[str]:
    try:
        with _scandir_relative(directory_ref) as entries:
            return _public_claim_names(directory_ref, entries, probe_clock=probe_clock)
    except OSError as error:
        reason = f"cannot list claim directory: {error.strerror or type(error).__name__}"
        raise SoftFileLockProtocolError(lock_file, None, reason) from error


def _read_claim_records(
    lock_file: str, directory_ref: tuple[str, int | None], names: list[str], *, probe_clock: bool
) -> dict[str, bytes]:
    # A contended scan can list a claim that is not yet cleanly readable, in two ways that both resolve on a brief
    # retry. Windows fails an open with EACCES while a peer's unlink holds the claim; once the deletion is pending,
    # _attempt_claim_read skips the claim outright, as another process's handle can keep the name listed for seconds.
    # On NFS, a peer that unlinks its own claim leaves this client's cached filehandle stale, so the next open returns
    # ESTALE rather than a clean ENOENT until the lookup revalidates against the server. Retrying re-runs the path
    # lookup, which turns the vanished claim into ENOENT (skip) or reads it if it still exists. A genuinely unreadable
    # record (a locked-down file, an EIO fault) still fails closed. The grace starts at the first pending result rather
    # than before the first attempt, so a slow first read cannot use up the window and leave no retry. One deadline
    # covers the scan: a deadline per claim let claims that clear one after another stretch a scan, and the caller's
    # timeout with it, by a grace each.
    records: dict[str, bytes] = {}
    deadline: float | None = None
    while True:
        pending: dict[str, OSError] = {}
        for name in names:
            record, error = _attempt_claim_read(lock_file, directory_ref, name)
            if error is not None:
                pending[name] = error
            elif record is not None:
                records[name] = record
        if not pending:
            return records
        if deadline is None:
            deadline = time.monotonic() + _CLAIM_READ_GRACE
        elif time.monotonic() >= deadline:
            break
        names = list(pending)
        time.sleep(_CLAIM_READ_RETRY)
    # A stale handle that outlives revalidation is a claim the server no longer has (RFC 1813 NFS3ERR_STALE): skip it
    # like ENOENT. Skipping a vanished claim overcounts contention at worst; it cannot free a held lock.
    if denied := [name for name, error in pending.items() if error.errno != ESTALE]:
        # Windows can deny opens after a peer removes the claim from the directory.
        listed: Final = set(_list_claim_names(lock_file, directory_ref, probe_clock=probe_clock))
        if still_listed := [name for name in denied if name in listed]:
            error = pending[still_listed[0]]
            reason = f"cannot read claim: {error.strerror or str(error) or type(error).__name__}"
            raise SoftFileLockProtocolError(lock_file, still_listed[0], reason) from error
    return records


def _attempt_claim_read(
    lock_file: str, directory_ref: tuple[str, int | None], name: str
) -> tuple[bytes | None, OSError | None]:
    # Return the record, or (None, None) when the claim has already gone, or (None, error) for an open the caller may
    # retry: a Windows EACCES while a peer's unlink holds the claim, or an NFS ESTALE from a peer unlinking its own
    # claim out from under this client's cached filehandle. Any other OSError is a real fault and fails closed.
    try:
        return _read_record(directory_ref, name, _CLAIM_RECORD_LIMIT), None
    except FileNotFoundError:
        return None, None
    except PermissionError as error:
        if sys.platform == "win32" and _is_unlinked(str(Path(directory_ref[0], name))):  # pragma: win32 cover
            return None, None
        return None, error
    except OSError as error:
        if error.errno == ESTALE:
            return None, error
        reason = f"cannot read claim: {error.strerror or str(error) or type(error).__name__}"
        raise SoftFileLockProtocolError(lock_file, name, reason) from error


def _public_claim_names(
    directory_ref: tuple[str, int | None], entries: Iterator[os.DirEntry[str]], *, probe_clock: bool
) -> list[str]:
    names: list[str] = []
    now: Callable[[], float | None] = (
        functools.cache(
            functools.partial(
                _filesystem_now, directory_ref, _claim_name("held", secrets.token_hex(_TOKEN_HEX_LENGTH // 2))
            )
        )
        if probe_clock
        else lambda: None
    )
    for entry in entries:
        if not entry.name.startswith("."):
            names.append(entry.name)
        elif _is_claim_probe_name(entry.name) or (
            (public_name := _private_public_name(entry.name)) is not None and _parse_claim_name(public_name) is not None
        ):
            _reclaim_private_record(directory_ref, entry.name, now)
    return sorted(names)


def _read_existing_claims(
    lock_file: str, directory: Path, *, probe_clock: bool = True, verify_server: bool = sys.platform != "win32"
) -> tuple[StrictSoftFileClaim, ...]:
    if not _OPEN_SUPPORTS_DIR_FD:  # pragma: lacks dir-fd
        try:
            pinned = _pin_claim_directory(directory)
        except FileNotFoundError:
            return ()
        except OSError as error:
            reason = f"cannot open claim directory: {error.strerror or type(error).__name__}"
            raise SoftFileLockProtocolError(lock_file, None, reason) from error
        with pinned:
            return _read_claims(
                lock_file, (os.fspath(directory), None), probe_clock=probe_clock, verify_server=verify_server
            )
    return _read_claims_through_descriptor(  # pragma: needs dir-fd
        lock_file, directory, probe_clock=probe_clock, verify_server=verify_server
    )


def _read_claims_through_descriptor(  # pragma: needs dir-fd
    lock_file: str, directory: Path, *, probe_clock: bool, verify_server: bool
) -> tuple[StrictSoftFileClaim, ...]:
    try:
        directory_fd = _open_claim_directory(os.fspath(directory))
    except FileNotFoundError:
        return ()
    except OSError as error:
        reason = f"cannot open claim directory: {error.strerror or type(error).__name__}"
        raise SoftFileLockProtocolError(lock_file, None, reason) from error
    try:
        return _read_claims(
            lock_file, (os.fspath(directory), directory_fd), probe_clock=probe_clock, verify_server=verify_server
        )
    finally:
        # A failed close has no result to report after the scan and proof cleanup.
        with contextlib.suppress(OSError):
            os.close(directory_fd)


def _parse_claim(
    lock_file: str,
    name: str,
    name_parts: tuple[StrictSoftFileClaimState, str],
    record: bytes,
) -> StrictSoftFileClaim:
    try:
        magic, token, pid_text, hostname_hex, start_text, trailing = record.decode("ascii").split("\n")
        pid = int(pid_text)
        start = int(start_text) if start_text else None
        hostname = bytes.fromhex(hostname_hex).decode("utf-8")
    except (UnicodeDecodeError, ValueError) as error:
        raise SoftFileLockProtocolError(lock_file, name, "malformed claim record") from error
    if not all((
        not trailing,
        magic == _CLAIM_MAGIC,
        token == name_parts[1],
        1 <= pid <= 2**31 - 1,
        str(pid) == pid_text,
        start is None or (start >= 0 and str(start) == start_text),
        hostname.encode().hex() == hostname_hex
        and hostname.isprintable()
        and not any(character.isspace() for character in hostname),
    )):
        raise SoftFileLockProtocolError(lock_file, name, "malformed claim record")
    return StrictSoftFileClaim(name=name, state=name_parts[0], token=token, pid=pid, hostname=hostname, start=start)


def _parse_claim_name(name: str) -> tuple[StrictSoftFileClaimState, str] | None:
    if not name.endswith(".claim"):
        return None
    parts = name.removesuffix(".claim").split("-")
    if len(parts) != _CLAIM_NAME_PART_COUNT or parts[0] not in _CLAIM_STATES or parts[1] != "v1":
        return None
    token = parts[2]
    if len(token) != _TOKEN_HEX_LENGTH or any(character not in "0123456789abcdef" for character in token):
        return None
    return cast("StrictSoftFileClaimState", parts[0]), token


def _claim_name(state: StrictSoftFileClaimState, token: str) -> str:
    return f"{state}-v1-{token}.claim"


def _claim_token_key(name: str) -> str:
    return name.removeprefix("held-").removeprefix("intent-")


def _claim_record(token: str) -> bytes:
    hostname_hex = host_name().encode().hex()
    start = process_start_token(os.getpid())
    start_text = "" if start is None else str(start)
    return f"{_CLAIM_MAGIC}\n{token}\n{os.getpid()}\n{hostname_hex}\n{start_text}\n".encode("ascii")


def _publish_record(
    public_path: str,
    record: bytes,
    mode: int,
    open_directory: Callable[[str], int] | None = None,
) -> BaseException | None:
    directory, public_name = os.path.split(public_path)
    directory = directory or os.curdir
    private_name = _private_record_name(public_name)
    directory_fd = (open_directory or _open_directory)(directory) if _OPEN_SUPPORTS_DIR_FD else None
    directory_ref = directory, directory_fd
    try:
        _publish_record_in_directory(directory_ref, (private_name, public_name), mode, record)
    except BaseException as publication_error:  # preserve publication and directory cleanup errors
        try:
            if directory_fd is not None:  # pragma: needs dir-fd
                os.close(directory_fd)
        except BaseException as close_error:  # ruff:ignore[blind-except]  # pragma: needs dir-fd  # preserve publication and directory cleanup errors
            _raise_cleanup_errors("strict publication directory cleanup failed", publication_error, close_error)
        raise
    if directory_fd is not None:  # pragma: needs dir-fd
        try:
            os.close(directory_fd)
        except BaseException as close_error:  # ruff:ignore[blind-except]  # a claim's caller recorded its path before publishing
            return close_error
    return None


def _publish_record_in_directory(
    directory_ref: tuple[str, int | None],
    names: tuple[str, str],
    mode: int,
    record: bytes,
) -> None:
    flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | _O_BINARY
    if (o_nofollow := getattr(os, "O_NOFOLLOW", None)) is not None:  # pragma: needs o-nofollow
        flags |= o_nofollow
    private_fd = _open_relative(directory_ref, names[0], flags, mode)
    private_identity: tuple[int, int] | None = None
    try:
        private_identity = _file_identity(os.fstat(private_fd))
        write_all(private_fd, record)
        _link_private_record(directory_ref, names, private_identity)
    except BaseException as publication_error:  # preserve publication and cleanup errors
        close_error, unlink_error = _close_and_unlink_private_record(
            directory_ref,
            names[0],
            private_fd,
            private_identity,
        )
        if close_error is not None or unlink_error is not None:
            _raise_cleanup_errors(
                "strict record publication cleanup failed",
                publication_error,
                close_error,
                unlink_error,
            )
        raise
    close_error, unlink_error = _close_and_unlink_private_record(
        directory_ref,
        names[0],
        private_fd,
        private_identity,
    )
    if close_error is not None or unlink_error is not None:
        _raise_record_finalization_errors(close_error, unlink_error)


def _close_and_unlink_private_record(
    directory_ref: tuple[str, int | None],
    private_name: str,
    private_fd: int,
    private_identity: tuple[int, int] | None,
) -> tuple[BaseException | None, BaseException | None]:
    close_error: BaseException | None = None
    try:
        os.close(private_fd)
    except BaseException as error:  # ruff:ignore[blind-except]  # returned for grouping with unlink failures
        close_error = error
    unlink_error: BaseException | None = None
    try:
        if private_identity is None:
            _unlink_relative(directory_ref, private_name)
        else:
            _unlink_relative_if_identity(directory_ref, private_name, private_identity)
    except FileNotFoundError:
        pass
    except BaseException as error:  # ruff:ignore[blind-except]  # returned for grouping with close failures
        unlink_error = error
    return close_error, unlink_error


def _link_private_record(
    directory_ref: tuple[str, int | None],
    names: tuple[str, str],
    private_identity: tuple[int, int],
) -> None:
    try:
        _link_or_find_committed(directory_ref, *names, private_identity)
    except FileNotFoundError as error:
        if _relative_identity(directory_ref, names[0]) is not None:
            raise
        msg = "private publication record was reclaimed"
        raise _PrivateRecordReclaimedError(msg) from error
    if _relative_identity(directory_ref, names[1]) == private_identity:
        return
    msg_0 = "private publication record was replaced"
    raise _PrivateRecordReclaimedError(msg_0)


def _raise_record_finalization_errors(
    close_error: BaseException | None,
    unlink_error: BaseException | None,
) -> None:
    errors = [error for error in (close_error, unlink_error) if error is not None]
    if len(errors) > 1:
        _raise_cleanup_errors("strict record finalization failed", errors[0], *errors[1:])
    raise errors[0]


def _private_record_name(public_name: str) -> str:
    return f".{public_name}{_PRIVATE_RECORD_MARKER}{secrets.token_hex(_PRIVATE_RECORD_RANDOM_HEX_LENGTH // 2)}.tmp"


def _private_public_name(private_name: str) -> str | None:
    if not private_name.startswith(".") or not private_name.endswith(_PRIVATE_RECORD_SUFFIX):
        return None
    public_name, marker, random_hex = private_name[1 : -len(_PRIVATE_RECORD_SUFFIX)].rpartition(_PRIVATE_RECORD_MARKER)
    if (
        marker != _PRIVATE_RECORD_MARKER
        or len(random_hex) != _PRIVATE_RECORD_RANDOM_HEX_LENGTH
        or any(character not in "0123456789abcdef" for character in random_hex)
    ):
        return None
    return public_name


def _is_claim_probe_name(name: str) -> bool:
    if not name.startswith(_CLAIM_PROBE_PREFIX) or not name.endswith(_PRIVATE_RECORD_SUFFIX):
        return False
    token = name[len(_CLAIM_PROBE_PREFIX) : -len(_PRIVATE_RECORD_SUFFIX)]
    return len(token) == _TOKEN_HEX_LENGTH and all(character in "0123456789abcdef" for character in token)


def _reclaim_sentinel_private_records(lock_file: str, path: Path) -> None:
    directory_ref = os.fspath(path.parent), None
    now = functools.cache(functools.partial(_filesystem_now, directory_ref, path.name))
    try:
        with os.scandir(path.parent) as entries:
            for entry in entries:
                if _private_public_name(entry.name) == path.name:
                    _reclaim_private_record(directory_ref, entry.name, now)
    except OSError as error:
        reason = f"cannot reclaim sentinel private records: {error.strerror or error}"
        raise SoftFileLockProtocolError(lock_file, None, reason) from error


def _reclaim_private_record(
    directory_ref: tuple[str, int | None], private_name: str, now: Callable[[], float | None]
) -> None:
    directory, directory_fd = directory_ref
    try:
        private_stat = (
            os.stat(private_name, dir_fd=directory_fd, follow_symlinks=False)
            if directory_fd is not None and _STAT_SUPPORTS_DIR_FD
            else Path(directory, private_name).lstat()
        )
    except FileNotFoundError:
        return
    if not stat.S_ISREG(private_stat.st_mode):
        msg = f"{Path(directory, private_name)} is not a regular private record"
        raise OSError(msg)
    if private_stat.st_nlink == 1 and (
        (current := now()) is None or current - private_stat.st_mtime < _PRIVATE_RECORD_GRACE
    ):
        return
    try:
        _unlink_private_record_once(directory_ref, private_name)
    except FileNotFoundError:
        pass
    except OSError as error:
        if error.errno not in {EACCES, EPERM}:
            raise


def _filesystem_now(directory_ref: tuple[str, int | None], public_name: str) -> float | None:
    # A network filesystem stamps mtimes with the server's clock, which can run seconds off this client's, so read "now"
    # as the mtime of a probe written like a record (liblockfile uses a read's atime). The probe has a private-record
    # name, so a scan reaps one left behind. Without a stamp no age is trusted.
    probe = _private_record_name(public_name)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | _O_BINARY | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = _open_relative(directory_ref, probe, flags, 0o600)
    except OSError:
        return None
    try:
        write_all(fd, b"\n")
        return os.fstat(fd).st_mtime
    except OSError:
        return None
    finally:
        with contextlib.suppress(OSError):
            os.close(fd)
        with contextlib.suppress(OSError):
            _unlink_private_record_once(directory_ref, probe)


def _unlink_private_record_once(directory_ref: tuple[str, int | None], private_name: str) -> None:
    directory, directory_fd = directory_ref
    if directory_fd is not None and _UNLINK_SUPPORTS_DIR_FD:  # pragma: needs dir-fd
        os.unlink(private_name, dir_fd=directory_fd)
    else:
        Path(directory, private_name).unlink()


def _relative_identity(directory_ref: tuple[str, int | None], name: str) -> tuple[int, int] | None:
    directory, directory_fd = directory_ref
    try:
        if directory_fd is not None and _STAT_SUPPORTS_DIR_FD:  # pragma: needs dir-fd
            path_stat = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
        else:  # pragma: win32 cover
            path_stat = Path(directory, name).lstat()
    except FileNotFoundError:
        return None
    return _file_identity(path_stat)


def _unlink_relative_if_identity(
    directory_ref: tuple[str, int | None],
    name: str,
    identity: tuple[int, int],
) -> None:
    if _relative_identity(directory_ref, name) == identity:
        with contextlib.suppress(FileNotFoundError):
            _unlink_relative(directory_ref, name)


def _open_relative(directory_ref: tuple[str, int | None], name: str, flags: int, mode: int) -> int:
    directory, directory_fd = directory_ref
    return (
        os.open(name, flags, mode, dir_fd=directory_fd)
        if directory_fd is not None
        else os.open(Path(directory, name), flags, mode)
    )


def _link_relative(directory_ref: tuple[str, int | None], source_name: str, destination_name: str) -> None:
    directory, directory_fd = directory_ref
    if directory_fd is not None and _LINK_SUPPORTS_DIR_FD:  # pragma: needs dir-fd
        _link_no_follow(source_name, destination_name, src_dir_fd=directory_fd, dst_dir_fd=directory_fd)
        return
    _link_no_follow(Path(directory, source_name), Path(directory, destination_name))  # pragma: win32 cover


def _link_or_find_committed(
    directory_ref: tuple[str, int | None],
    source_name: str,
    destination_name: str,
    source_identity: tuple[int, int] | None = None,
) -> None:
    try:
        _link_relative(directory_ref, source_name, destination_name)
    except FileExistsError:
        # NFS retransmits a LINK whose reply was lost and reports EEXIST for the committed link; as in open(2) NOTES
        # and liblockfile, it succeeded when the destination names the source. Prefer the fstat-ed identity: a peer's
        # scan reaps a private record once it has two links, so its name may already resolve to nothing.
        if (identity := _relative_identity(directory_ref, destination_name)) is None or identity != (
            source_identity or _relative_identity(directory_ref, source_name)
        ):
            raise


def _link_no_follow(
    source: str | Path,
    destination: str | Path,
    *,
    src_dir_fd: int | None = None,
    dst_dir_fd: int | None = None,
) -> None:
    if not _HAS_LINK:
        # No os.link at all (Termux/Android): report it as unsupported like a filesystem that refuses hard links, so
        # the acquire path raises SoftFileLockProtocolError instead of a bare AttributeError.
        msg = "os.link is unavailable on this platform"
        raise NotImplementedError(msg)
    # The source is a private record this process created with O_CREAT | O_EXCL, so follow_symlinks guards nothing an
    # attacker can reach. Pass the option only when the runtime honors it: PyPy advertises it through
    # os.supports_follow_symlinks yet its linkat rejects it with EINVAL, so probe once rather than trust the set.
    if _LINK_HONORS_FOLLOW_SYMLINKS:
        os.link(source, destination, src_dir_fd=src_dir_fd, dst_dir_fd=dst_dir_fd, follow_symlinks=False)
    else:  # pragma: lacks link-follow-symlinks
        os.link(source, destination, src_dir_fd=src_dir_fd, dst_dir_fd=dst_dir_fd)


def _unlink_relative(directory_ref: tuple[str, int | None], name: str) -> None:
    directory, directory_fd = directory_ref
    if directory_fd is not None and _UNLINK_SUPPORTS_DIR_FD:  # pragma: needs dir-fd
        os.unlink(name, dir_fd=directory_fd)
    elif sys.platform == "win32":  # pragma: win32 cover
        _unlink_in_directory(Path(directory), name)
    else:
        Path(directory, name).unlink()  # pragma: win32 no cover


def _link_no_replace(directory: Path, source_name: str, destination_name: str) -> BaseException | None:
    if _LINK_SUPPORTS_DIR_FD:  # pragma: needs dir-fd
        directory_fd = _open_claim_directory(str(directory))
        try:
            _link_or_find_committed((str(directory), directory_fd), source_name, destination_name)
        except BaseException as link_error:  # preserve link and directory cleanup errors
            try:
                os.close(directory_fd)
            except BaseException as close_error:  # ruff:ignore[blind-except]  # preserve link and directory cleanup errors
                _raise_cleanup_errors("strict link directory cleanup failed", link_error, close_error)
            raise
        try:
            os.close(directory_fd)
        except BaseException as close_error:  # ruff:ignore[blind-except]  # the caller recorded the held path before linking
            return close_error
        return None
    with _pin_claim_directory(directory):  # pragma: win32 cover
        _link_or_find_committed((str(directory), None), source_name, destination_name)
    return None  # pragma: win32 cover


def _unlink_owner_path(path: str) -> BaseException | None:
    directory, name = os.path.split(path)
    with _pin_claim_directory(claim_directory := Path(directory or os.curdir)):
        return _unlink_in_directory(claim_directory, name)


def _unlink_owner_paths(paths: tuple[str, ...]) -> tuple[list[str], list[BaseException]]:
    results = [(path, _unlink_owner_path_result(path)) for path in paths]
    return [path for path, (removed, _) in results if not removed], [
        error for _, (_, error) in results if error is not None
    ]


def _unlink_owner_path_result(path: str) -> tuple[bool, BaseException | None]:
    try:
        cleanup_error = _unlink_owner_path(path)
    except FileNotFoundError:
        return True, None
    except BaseException as error:  # ruff:ignore[blind-except]  # keep ownership when unlink did not commit
        return False, error
    return True, cleanup_error


def _raise_recorded_errors(message: str, errors: list[BaseException]) -> None:
    if len(errors) > 1:
        _raise_cleanup_errors(message, errors[0], *errors[1:])
    raise errors[0]


def _unlink_in_directory(directory: Path, name: str) -> BaseException | None:
    if _UNLINK_SUPPORTS_DIR_FD:  # pragma: needs dir-fd
        directory_fd = _open_claim_directory(str(directory))
        try:
            os.unlink(name, dir_fd=directory_fd)
        except BaseException as unlink_error:  # preserve unlink and directory cleanup errors
            try:
                os.close(directory_fd)
            except BaseException as close_error:  # ruff:ignore[blind-except]  # preserve unlink and directory cleanup errors
                _raise_cleanup_errors("strict unlink directory cleanup failed", unlink_error, close_error)
            raise
        try:
            os.close(directory_fd)
        except BaseException as close_error:  # ruff:ignore[blind-except]  # caller commits the removed path before raising
            return close_error
        return None
    if sys.platform != "win32":  # pragma: win32 no cover
        Path(directory / name).unlink()
        return None
    retry_delay = 0.001  # pragma: win32 cover
    for attempt in range(_UNLINK_MAX_RETRIES):  # pragma: win32 cover
        if (error := _unlink_error(directory / name)) is None:
            return None
        if error.errno not in {EACCES, EPERM} or attempt == _UNLINK_MAX_RETRIES - 1:
            raise error
        time.sleep(retry_delay)
        retry_delay *= 2
    return None  # pragma: no cover  # the final retry returns or raises


def _unlink_error(path: Path) -> OSError | None:  # pragma: win32 cover
    try:
        path.unlink()
    except OSError as error:
        return error
    return None


def _pin_claim_directory(directory: Path) -> contextlib.ExitStack[bool | None]:
    return _pin_directories(directory.parent, directory)


def _pin_directories(*directories: Path) -> contextlib.ExitStack[bool | None]:
    # Windows has no dir_fd, so each operation resolves every path component by name again. An open handle on a
    # directory blocks a rename of it, and with it a swap for a junction (a directory that points at another directory),
    # until the operation ends. Other platforms work through descriptors and hold nothing.
    stack: contextlib.ExitStack[bool | None] = contextlib.ExitStack()
    if sys.platform == "win32":  # pragma: win32 cover
        with stack:
            for directory in directories:
                stack.callback(_unpin_directory, _pin_directory(str(directory)))
            return stack.pop_all()
    return stack  # pragma: win32 no cover


def _open_directory(directory: str, dir_fd: int | None = None) -> int:  # pragma: needs dir-fd
    return os.open(directory, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0), dir_fd=dir_fd)


def _open_claim_directory(directory: str) -> int:  # pragma: needs dir-fd
    # O_NOFOLLOW applies to the last path component. A process that can rename entries in the lock's directory can
    # swap the coordination directory for a symlink after any check of its name, so open the claim directory relative
    # to a descriptor of the coordination directory: a symlink at either level then fails the open.
    coordination_directory, name = os.path.split(directory)
    coordination_fd = _open_directory(coordination_directory)
    try:
        return _open_directory(name, coordination_fd)
    finally:
        # The walk wrote nothing through it, and an error here would leak the descriptor this function returns.
        with contextlib.suppress(OSError):
            os.close(coordination_fd)


def _scandir_relative(
    directory_ref: tuple[str, int | None],
) -> contextlib.AbstractContextManager[Iterator[os.DirEntry[str]]]:
    directory, directory_fd = directory_ref
    if directory_fd is not None and _SCANDIR_SUPPORTS_FD:  # pragma: needs dir-fd
        return os.scandir(directory_fd)
    return os.scandir(directory)  # pragma: lacks dir-fd


def _lstat_relative(directory_ref: tuple[str, int | None], name: str) -> os.stat_result:
    directory, directory_fd = directory_ref
    if directory_fd is not None and _STAT_SUPPORTS_DIR_FD:  # pragma: needs dir-fd
        return os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
    return Path(directory, name).lstat()


def _read_record(directory_ref: tuple[str, int | None], name: str, limit: int) -> bytes:
    fd, record = _open_record(directory_ref, name, limit, deletable=True)
    os.close(fd)
    return record


def _open_record(
    directory_ref: tuple[str, int | None], name: str, limit: int, *, deletable: bool = False
) -> tuple[int, bytes]:
    path = Path(directory_ref[0], name)
    path_stat = _lstat_relative(directory_ref, name)
    if not stat.S_ISREG(path_stat.st_mode):
        msg = f"{path} is not a regular file"
        raise OSError(msg)
    if deletable and sys.platform == "win32":  # pragma: win32 cover
        # A claim's owner must be able to unlink it while another process reads it. The sentinel stays for the lock's
        # lifetime, so it keeps os.open.
        fd = _open_deletable_for_reading(str(path))
    else:
        flags = os.O_RDONLY | _O_BINARY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
        fd = _open_relative(directory_ref, name, flags, 0)
    try:
        record = _read_opened_record(fd, path, path_stat, limit)
    except BaseException as read_error:  # preserve read and descriptor cleanup errors
        try:
            os.close(fd)
        except BaseException as close_error:  # ruff:ignore[blind-except]  # preserve read and descriptor cleanup errors
            _raise_cleanup_errors("strict record read cleanup failed", read_error, close_error)
        raise
    return fd, record


def _read_opened_record(fd: int, path: Path, path_stat: os.stat_result, limit: int) -> bytes:
    opened_stat = os.fstat(fd)
    if not stat.S_ISREG(opened_stat.st_mode):
        msg = f"{path} is not a regular file"
        raise OSError(msg)
    if _file_identity(opened_stat) != _file_identity(path_stat):
        msg = f"{path} changed while opening"
        raise OSError(msg)
    record = os.read(fd, limit + 1)
    if len(record) > limit:
        msg = f"{path} exceeds {limit} bytes"
        raise OSError(msg)
    return record


def _ensure_claim_directory(lock_file: str, directory: Path, mode: int) -> None:
    if not (_OPEN_SUPPORTS_DIR_FD and _MKDIR_SUPPORTS_DIR_FD):  # pragma: lacks dir-fd
        try:
            pinned = _pin_directories(directory.parent)
        except OSError as error:
            raise SoftFileLockProtocolError(lock_file, None, f"cannot open {directory.parent}") from error
        with pinned:
            _ensure_protocol_directory(lock_file, directory, mode)
    else:  # pragma: needs dir-fd
        # mkdir follows a symlink in every component but the last, so create against a descriptor of the real parent.
        try:
            coordination_fd = _open_directory(os.fspath(directory.parent))
        except OSError as error:
            raise SoftFileLockProtocolError(lock_file, None, f"cannot open {directory.parent}") from error
        try:
            _ensure_protocol_directory(lock_file, directory, mode, coordination_fd)
        finally:
            with contextlib.suppress(OSError):
                os.close(coordination_fd)


def _ensure_protocol_directory(lock_file: str, directory: Path, mode: int, parent_fd: int | None = None) -> None:
    try:
        if parent_fd is None:
            directory.mkdir(mode)
        else:  # pragma: needs dir-fd
            os.mkdir(directory.name, mode, dir_fd=parent_fd)
    except FileExistsError:
        try:
            found = _lstat_relative((os.fspath(directory.parent), parent_fd), directory.name)
            owner = foreign_directory_owner(
                found, directory.parent.stat() if parent_fd is None else os.fstat(parent_fd)
            )
        except OSError as error:
            raise SoftFileLockProtocolError(lock_file, None, f"cannot inspect {directory}") from error
        if (
            not stat.S_ISDIR(found.st_mode)
            or getattr(found, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT
        ):
            raise SoftFileLockProtocolError(lock_file, None, f"{directory} is not a real directory") from None
        if owner is not None:  # pragma: needs foreign-sticky-tmp
            reason = f"{directory} belongs to uid {owner}, not to this process's account or the parent's owner"
            raise SoftFileLockProtocolError(lock_file, None, reason) from None
    _settle_directory_mode(lock_file, directory, mode, parent_fd)


def _settle_directory_mode(lock_file: str, directory: Path, mode: int, parent_fd: int | None) -> None:
    # The umask or a default ACL can leave the directory wider than the mode, now or in an earlier release, so narrow a
    # directory this account owns to the mode. The sticky bit (a directory flag that lets only a file's owner remove it)
    # then protects each claim from other accounts even where the mode admits them. Windows has neither mode bits nor a
    # sticky bit.
    if sys.platform != "win32":  # pragma: win32 no cover
        try:
            fd = os.open(
                directory.name if parent_fd is not None else os.fspath(directory),
                os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0),
                dir_fd=parent_fd,
            )
        except OSError as error:
            raise SoftFileLockProtocolError(lock_file, None, f"cannot open {directory}") from error
        try:
            found = os.fstat(fd)
            settled = found.st_mode & 0o7000 | found.st_mode & mode & 0o777 | stat.S_ISVTX
            if found.st_uid == os.geteuid() and settled != stat.S_IMODE(found.st_mode):
                os.fchmod(fd, settled)
        except OSError as error:
            raise SoftFileLockProtocolError(lock_file, None, f"cannot set the mode of {directory}") from error
        finally:
            os.close(fd)


def _validate_force_break_name(name: str) -> None:
    invalid_component = not name or name.startswith(".") or name in {os.curdir, os.pardir}
    if invalid_component or any(separator in name for separator in ("/", "\\", "\x00")):
        msg = "claim_name must be one public claim basename"
        raise ValueError(msg)


def _require_exact_name(directory: Path, name: str) -> None:
    directory_fd = _open_claim_directory(os.fspath(directory)) if _OPEN_SUPPORTS_DIR_FD else None
    try:
        with _scandir_relative((os.fspath(directory), directory_fd)) as entries:
            if not any(entry.name == name for entry in entries):
                raise FileNotFoundError(ENOENT, os.strerror(ENOENT), name)
    finally:
        if directory_fd is not None:  # pragma: needs dir-fd
            with contextlib.suppress(OSError):
                os.close(directory_fd)


def _raise_if_hard_links_unsupported(lock_file: str, error: NotImplementedError | OSError) -> None:
    unsupported = (
        isinstance(error, NotImplementedError)
        or error.errno in _HARD_LINK_UNSUPPORTED_ERRNOS
        or (getattr(error, "winerror", None) in _WINDOWS_HARD_LINK_UNSUPPORTED)
    )
    if unsupported:
        reason = "filesystem does not support atomic no-replace hard-link publication"
        raise SoftFileLockProtocolError(lock_file, None, reason) from error


def _file_identity(st: os.stat_result) -> tuple[int, int]:
    return st.st_dev, st.st_ino


__all__ = [
    "StrictSoftFileClaim",
    "StrictSoftFileClaimState",
    "StrictSoftFileLock",
]
