from __future__ import annotations

__lazy_modules__ = [
    "contextlib",
    "filelock._error",
    "filelock._identity",
    "filelock._soft",
    "filelock._util",
    "math",
    "secrets",
    "threading",
    "weakref",
]

import os
import secrets
import sys
import time
from contextlib import suppress
from dataclasses import dataclass
from math import isfinite
from threading import Event, Thread, current_thread
from typing import TYPE_CHECKING, Final, Literal, cast
from weakref import WeakMethod

from filelock._api import _check_timeout_max, _seconds
from filelock._error import LeaseSettingsMismatch
from filelock._identity import owner_is_stale
from filelock._marker import MarkerSoftFileLock, OwnerMode, OwnerRecord, parse_marker
from filelock._soft import _read_lock_file
from filelock._util import pin_inode, touch

if TYPE_CHECKING:
    from collections.abc import Callable, Mapping
    from typing import Unpack

    from filelock._api import LockOptions, _ExtraValue

#: ``evicted`` is a :class:`~filelock.SoftReadWriteLock` loss: a peer waited out the stale threshold and committed a
#: generation without this holder.
CompromiseReason = Literal["marker-missing", "owner-changed", "refresh-failed", "evicted"]

_RefreshOutcome = Literal["ok", "transient", "marker-missing", "owner-changed"]
_DEFAULT_LEASE_DURATION: Final[float] = 30.0


@dataclass(frozen=True)
class LeaseCompromise:
    """Why a held lease stopped being this process's to hold, and the error that ended its refresh, if any."""

    lock_file: str
    token: str
    reason: CompromiseReason
    error: Exception | None = None


@dataclass(frozen=True)
class _Heartbeat:
    """A running heartbeat and the event that stops it, which only ever exist together."""

    thread: Thread
    stop: Event


@dataclass
class _LeaseClaim:
    """The state of one claim: its token, its heartbeat, and how that claim was lost."""

    token: str | None = None
    compromise: LeaseCompromise | None = None
    heartbeat: _Heartbeat | None = None


class SoftFileLease(MarkerSoftFileLock):
    """
    Existence lock whose claim expires, so a peer may take it while the previous holder still runs.

    A lease trades mutual exclusion for progress. The holder publishes a claim and refreshes it every
    ``heartbeat_interval`` seconds; a contender takes the marker once it has seen no refresh for ``lease_duration``
    seconds of its own monotonic clock, so clock skew between hosts cannot expire a live claim. Nothing stops the
    expired holder: it keeps running, and it keeps using whatever the lock protects. Treat the lease as a hint about
    who *should* be working, not as a guarantee that only one worker is.

    To make a protected resource reject a superseded holder, that resource must be linearizable and must fence on a
    monotonic generation it controls. :attr:`token` names a claim; it does not fence one. Where overlap is unacceptable,
    use :class:`StrictSoftFileLock <filelock.StrictSoftFileLock>` instead.

    Every contender for a path must agree on ``lease_duration``. A contender that finds a claim published under a
    different duration raises :class:`LeaseSettingsMismatch <filelock.LeaseSettingsMismatch>` rather than apply its own
    expiry to a peer that never agreed to it.

    Expiry reclaims less on Windows, which refuses to rename or delete a file another process holds open. A peer there
    takes an expired claim only once the previous holder's process exits and its handle closes; a holder that lives on
    but stops refreshing keeps the marker. Unix reclaims the marker either way.

    ``on_compromise`` fires from the heartbeat thread when a refresh fails, or when the marker vanishes or names another
    owner. The holder should stop touching the protected resource when it runs. Because it runs on that thread, a
    ``release()`` inside it only takes effect when the lease was built with ``thread_local=False``; the default
    thread-local context hides the claim from every thread but the one that acquired it, so the release does nothing.
    Signal the acquiring thread instead when the context stays thread-local.

    .. versionadded:: 3.30.0

    """

    _owner_mode: OwnerMode = "lease"

    #: lease_duration replaces the legacy age-based lifetime, so accepting both would give one lock two expiry clocks.
    _lifetime_supported: bool = False
    _lifetime_unsupported_reason: str = "lease_duration sets when a lease expires"

    def __init__(
        self,
        lock_file: str | os.PathLike[str],
        *,
        lease_duration: float = _DEFAULT_LEASE_DURATION,
        heartbeat_interval: float | None = None,
        on_compromise: Callable[[LeaseCompromise], None] | None = None,
        **kwargs: Unpack[LockOptions],
    ) -> None:
        """
        Create a lease.

        :param lease_duration: seconds a contender must see the marker go unrefreshed before it may take the claim.
            Every contender for the path must pass the same value.
        :param heartbeat_interval: seconds between refreshes. Defaults to a third of ``lease_duration``, leaving room
            for two missed refreshes before a peer may take the claim. Must be shorter than ``lease_duration`` and at
            most :data:`threading.TIMEOUT_MAX`.
        :param on_compromise: called from the heartbeat thread with a :class:`LeaseCompromise` when the claim is lost.
        :param kwargs: every other :class:`BaseFileLock <filelock.BaseFileLock>` option, ``timeout`` and ``mode`` among
            them. The metaclass passes them all by keyword, and taking them here lets
            :class:`AsyncSoftFileLease <filelock.AsyncSoftFileLease>` add the async plumbing a fixed signature would
            hide.

        """
        # A plain float: the marker records its repr, which NumPy's float64 spells in a form no parser reads.
        if not isfinite(lease_duration := _seconds("lease_duration", lease_duration)) or lease_duration <= 0:
            msg = f"lease_duration must be positive and finite, got {lease_duration!r}"
            raise ValueError(msg)
        heartbeat_interval = (
            lease_duration / 3 if heartbeat_interval is None else _seconds("heartbeat_interval", heartbeat_interval)
        )
        if not 0 < heartbeat_interval < lease_duration:
            msg = f"heartbeat_interval must be positive and below lease_duration, got {heartbeat_interval!r}"
            raise ValueError(msg)
        _check_timeout_max("heartbeat_interval", heartbeat_interval)
        super().__init__(lock_file, **kwargs)
        self._lease_duration = lease_duration
        self._heartbeat_interval = heartbeat_interval
        self._on_compromise = on_compromise
        # Two threads lazily creating a shared claim could each get one, and release() would miss the heartbeat's.
        if not self.is_thread_local():
            self._context.lease_claim = _LeaseClaim()

    def _singleton_extra_mismatches(self, kwargs: Mapping[str, _ExtraValue], /) -> dict[str, tuple[str, str]]:
        mismatches = super()._singleton_extra_mismatches(kwargs)
        lease_duration: Final = _seconds(
            "lease_duration", cast("float", kwargs.get("lease_duration", _DEFAULT_LEASE_DURATION))
        )
        if lease_duration != self._lease_duration:
            mismatches["lease_duration"] = (str(lease_duration), str(self._lease_duration))
        # An omitted heartbeat_interval resolves as in __init__; a differing duration is already reported above.
        heartbeat_interval: Final = (
            self._lease_duration / 3
            if (passed := kwargs.get("heartbeat_interval")) is None
            else _seconds("heartbeat_interval", cast("float", passed))
        )
        if heartbeat_interval != self._heartbeat_interval:
            mismatches["heartbeat_interval"] = (str(passed), str(self._heartbeat_interval))
        # A callback compares by identity, as on_acquired does: two equal callables can close over different state.
        if (on_compromise := kwargs.get("on_compromise")) is not self._on_compromise:
            mismatches["on_compromise"] = (str(on_compromise), str(self._on_compromise))
        return mismatches

    @property
    def _claim(self) -> _LeaseClaim:
        # Per hold, so another thread's failed acquire cannot stop the holder's heartbeat.
        state: Final = self._hold_state()
        if (claim := state.lease_claim) is None:
            claim = state.lease_claim = _LeaseClaim()
        return claim

    @property
    def lease_duration(self) -> float:
        """The staleness in seconds after which a contender may take this claim."""
        return self._lease_duration

    @property
    def token(self) -> str | None:
        """
        The token naming the claim this process published.

        :returns: the token while the lease is held, ``None`` otherwise. It identifies a claim; it does not fence one.

        """
        return self._claim.token

    @property
    def compromise(self) -> LeaseCompromise | None:
        """
        The loss of claim the heartbeat observed.

        The value outlives ``release()``, so a holder can check after leaving the ``with`` block whether its work may
        have overlapped a successor; the next acquisition resets it, as :attr:`SoftReadWriteLock.compromise
        <filelock.SoftReadWriteLock.compromise>` does.

        :returns: the :class:`LeaseCompromise` of the latest claim, or ``None`` while that claim holds or held to its
            release

        """
        return self._claim.compromise

    def _acquire(self) -> None:
        claim = self._claim
        self._stop_heartbeat()  # no earlier claim's heartbeat outlives the acquisition of the next one
        # The published record reads the token, so it exists before the marker is written and goes back on failure.
        claim.token = token = secrets.token_hex(16)
        claim.compromise = None
        try:
            super()._acquire()
        except BaseException:
            claim.token = None
            raise
        # The context is thread-local by default, so the heartbeat thread cannot read the descriptor this one just
        # published, nor the claim this one owns. Hand it the fd, the inode it verified and the claim instead.
        if (fd := self._context.lock_file_fd) is not None and (
            identity := self._context.lock_file_fd_identity
        ) is not None:
            self._start_heartbeat(claim, fd, identity, token)
        else:
            claim.token = None

    def _release(self) -> None:
        self._stop_heartbeat()
        self._claim.token = None
        super()._release()

    def _reset_after_fork_in_child(self) -> None:  # pragma: forked child
        super()._reset_after_fork_in_child()
        # As in __init__: two child threads lazily creating a shared claim could each get one.
        if not self.is_thread_local():
            self._context.lease_claim = _LeaseClaim()

    def _published_record(self) -> OwnerRecord:
        return super()._published_record()._replace(token=self._claim.token, lease_duration=self._lease_duration)

    def _try_break_stale_lock(self) -> None:
        # A break can fail for reasons a contender must ride out rather than raise on: a peer broke the marker first,
        # or Windows refuses to rename a file whose holder still has it open. Poll again instead.
        with suppress(OSError), pin_inode(self._lock_path) as pinned:
            self._try_break_pinned_lock(pinned)

    def _try_break_pinned_lock(self, pinned: int | None) -> None:
        if (peer := self._read_peer()) is None:
            # Not a readable protocol 2 lease record: a partial write, a foreign or legacy protocol 1 marker, or the
            # strict sentinel. The base self-heal evicts a genuinely malformed marker once it ages past the grace
            # window and leaves a legitimate legacy or strict holder in place, so a corrupt marker no longer wedges
            # every lease contender until its own timeout.
            super()._try_break_stale_lock()
            return
        owner, marker = peer
        if pinned not in {None, marker.st_ino}:  # pragma: win32 no cover  # Windows pins nothing
            return  # replaced before the read, so nothing keeps the number read from reuse
        # Duration governs age-based expiry; a dead owner has no lease left to disagree with.
        if owner.mode != "unknown" and owner_is_stale(owner.pid, owner.hostname, owner.start):
            self._take_over(marker, owner.pid)
            return
        # Only a peer that published a lease agreed to be superseded by age; an exclusive owner is reclaimed only once
        # it is provably dead, and a contract this version does not know never is. Raise the mismatch outside the read
        # so the suppression cannot swallow it.
        if owner.mode == "lease" and owner.lease_duration != self._lease_duration:
            msg = (
                f"{self.lock_file} holds a lease of {owner.lease_duration!r}s but this contender configured "
                f"{self._lease_duration!r}s; every contender for a path must agree on lease_duration"
            )
            raise LeaseSettingsMismatch(msg)
        if owner.mode == "lease" and self._marker_unchanged_for(marker) >= self._lease_duration:
            self._take_over(marker, None)

    def _read_peer(self) -> tuple[OwnerRecord, os.stat_result] | None:
        with suppress(OSError, ValueError):
            content, marker = _read_lock_file(self._lock_path)
            if (owner := parse_marker(content)) is not None:
                return owner, marker
        return None

    def _start_heartbeat(self, claim: _LeaseClaim, fd: int, identity: tuple[int, int], token: str) -> None:
        # The thread watches the event it was handed rather than whatever the claim names later: a heartbeat that
        # outlives its join timeout would otherwise adopt the next acquisition's event and never stop.
        stop: Final = Event()
        loop: Final = _RefreshLoop(
            # A strong reference from a running thread would keep a dropped lease alive, so __del__ would never release
            # it and its marker would outlive the process. The callback stops a heartbeat that __del__ could not reach.
            report=WeakMethod(self._report_compromise, lambda _: stop.set()),
            lock_file=self._lock_path,
            claim=claim,
            fd=fd,
            identity=identity,
            token=token,
            stop=stop,
            interval=self._heartbeat_interval,
            duration=self._lease_duration,
        )
        thread: Final = Thread(target=loop.run, name=f"filelock-lease-{os.getpid()}", daemon=True)
        # Record the heartbeat before starting the thread so a release racing this acquire on a shared,
        # non-thread-local claim always sees it and sets the stop event; the thread then exits at its first wait
        # instead of outliving the release. A start that raises leaves the unstarted thread for _stop_heartbeat.
        claim.heartbeat = _Heartbeat(thread, stop)
        thread.start()

    def _stop_heartbeat(self) -> None:
        claim = self._claim
        if (heartbeat := claim.heartbeat) is None:
            return
        heartbeat.stop.set()
        claim.heartbeat = None
        # An unstarted thread has nothing to join, on_compromise may release from the heartbeat thread itself, and
        # during finalization a daemon thread is frozen, so joining it raises PythonFinalizationError (3.14+).
        if heartbeat.thread.ident is not None and heartbeat.thread is not current_thread() and not sys.is_finalizing():
            heartbeat.thread.join(timeout=self._heartbeat_interval)

    def _report_compromise(
        self,
        claim: _LeaseClaim,
        reason: CompromiseReason,
        error: OSError | None,
        token: str,
    ) -> None:
        # Record it on the claim this heartbeat serves, not on self._claim: a thread-local claim read from the
        # heartbeat thread is a different, empty one, so the holder would never see the loss it is being told about.
        # The token is the one this thread published, not claim.token, which a release may already have cleared.
        claim.compromise = LeaseCompromise(lock_file=self.lock_file, token=token, reason=reason, error=error)
        if self._on_compromise is not None:
            self._on_compromise(claim.compromise)


@dataclass(frozen=True)
class _RefreshLoop:
    """What a heartbeat thread needs to refresh one claim, holding the lease itself only weakly."""

    report: WeakMethod[Callable[[_LeaseClaim, CompromiseReason, OSError | None, str], None]]
    lock_file: str
    claim: _LeaseClaim
    fd: int
    identity: tuple[int, int]
    token: str
    stop: Event
    interval: float
    duration: float

    def run(self) -> None:
        # A transient error (ESTALE / EIO on the NFS-style filesystems a lease targets) is not a loss. Like restic, call
        # the claim unrefreshable once failures run long enough that a contender could take it before the next success.
        last_success, missing = time.monotonic(), False
        while not self.stop.wait(self.interval):
            outcome, error = self._refresh()
            if outcome == "ok":
                last_success, missing = time.monotonic(), False
                continue
            # A peer's stale break exchanges a live marker aside for the length of its check, so only a second miss or
            # change in a row is a loss.
            if outcome in {"marker-missing", "owner-changed"} and not missing:
                missing = True
                continue
            if outcome == "transient" and time.monotonic() - last_success < self.duration - self.interval:
                continue
            if (report := self.report()) is not None:
                report(self.claim, "refresh-failed" if outcome == "transient" else outcome, error, self.token)
            return

    def _refresh(self) -> tuple[_RefreshOutcome, OSError | None]:
        try:
            st = os.lstat(self.lock_file)
        except FileNotFoundError as error:
            return self._refresh_unnamed(error)
        except OSError as error:
            return "transient", error
        # A peer that took the expired claim replaced the marker, so the pathname now names its inode, not ours.
        if (st.st_dev, st.st_ino) != self.identity:
            return "owner-changed", None
        return self._touch()

    def _refresh_unnamed(self, name_error: FileNotFoundError) -> tuple[_RefreshOutcome, OSError | None]:
        # A renamed directory takes the marker with it; the name is gone but the inode behind the descriptor keeps its
        # link.
        try:
            linked = os.fstat(self.fd).st_nlink > 0
        except OSError as error:
            return "transient", error
        if not linked:
            return "marker-missing", name_error
        return self._touch()

    def _touch(self) -> tuple[_RefreshOutcome, OSError | None]:
        try:
            touch(self.lock_file, fd=self.fd)
        except OSError as error:
            return "transient", error
        return "ok", None


__all__ = [
    "CompromiseReason",
    "LeaseCompromise",
    "SoftFileLease",
    "_LeaseClaim",
]
