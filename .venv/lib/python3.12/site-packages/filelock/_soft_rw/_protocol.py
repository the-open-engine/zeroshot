"""
The generation-log protocol behind :class:`~filelock.SoftReadWriteLock`.

The lock's whole state is a sequence of immutable snapshots, ``gen/<N>``, each naming the writer and the readers that
hold the lock at generation ``N``. Every transition is one atomic publication of the next snapshot: a participant reads
the latest generation, derives the successor it wants, and links a fully written temporary file to ``gen/<N+1>``. The
link fails if a peer got there first, so the participant re-reads and tries again. No step is ever half done: a crash
leaves at most an unlinked temporary file, never a state that another participant must repair before it can proceed.

Liveness rests on a nonce, not a clock. Each participant keeps ``holders/<token>`` and rewrites its nonce every
heartbeat. A contender records the bytes it read there and the moment it read them on its own monotonic clock; a
member whose bytes have not changed for ``stale_threshold`` is evicted by committing a snapshot without it. Two hosts
never compare clocks, so a skewed file server or a suspended virtual machine cannot make a live holder look dead.

The generation number is monotonic and unique to one transition within one incarnation of the log, so the generation at
which a hold was granted fences it: a resource that rejects anything carrying a lower generation than the highest it has
accepted refuses a holder that paused past its expiry and resumed. A random epoch file names the incarnation, so a
participant that outlives a removal of the log reads the new one instead of continuing the old numbering into it.
"""

from __future__ import annotations

__lazy_modules__ = ["contextlib", "filelock._error", "filelock._identity", "math", "pathlib", "secrets"]

import os
import secrets
from contextlib import suppress
from dataclasses import dataclass
from math import isfinite
from pathlib import Path
from typing import TYPE_CHECKING, Final, Literal, Protocol

from filelock._error import SoftFileLockProtocolError
from filelock._identity import host_name

if TYPE_CHECKING:
    from collections.abc import Callable

Mode = Literal["read", "write"]
HeartbeatOutcome = Literal["ok", "lost", "transient"]

_PROTOCOL: Final[str] = "filelock-rw/1"
_GENERATION_DIGITS: Final[int] = 20
_TOKEN_HEX_LENGTH: Final[int] = 32
#: A snapshot line is ``reader=`` plus a token, so this bounds a hostile record without capping real reader counts.
_MAX_RECORD_SIZE: Final[int] = 1 << 20
#: Snapshots older than this many generations behind the latest are removed by whoever commits. The window is also how
#: far ``latest`` probes past a gap before it trusts that nothing newer exists; ``gen/HEAD``, not the window, is what
#: keeps a listing served further behind from passing an old generation off as the head.
_RETAINED_GENERATIONS: Final[int] = 64
_GENERATIONS_DIRECTORY: Final[str] = "gen"
#: A random token created once per log: removing ``gen/`` removes it, so a participant can tell a log that was wiped and
#: restarted from the one it remembers.
_EPOCH_NAME: Final[str] = "epoch"
#: The epoch plus a copy of the newest snapshot, renamed into place after each commit. NFS revalidates a file opened by
#: name but may serve a listing from cache, so this is the head a stale client cannot miss; it may lag, never lead.
_HEAD_NAME: Final[str] = "HEAD"
_HOLDERS_DIRECTORY: Final[str] = "holders"
_TEMPORARY_PREFIX: Final[str] = ".commit-"


class TreeReplacedError(RuntimeError):
    """
    Raised by a ``Files`` implementation when a protocol directory is not a real directory (a symlink, a junction, a
    file) or belongs to another account.
    """


class Files(Protocol):
    """The filesystem operations the protocol runs on; :mod:`._storage` implements them over :mod:`os`."""

    def read(self, path: str) -> bytes | None:
        """The file's bytes, ``None`` when it does not exist, or ``b""`` when it is not a regular file."""

    def create(self, path: str, data: bytes, *, durable: bool = False) -> tuple[int, int]:
        """
        Create the file exclusively with the whole record written; raise ``FileExistsError`` when it exists.

        The returned device and inode (the kernel's identity for a file, independent of its name) let ``overwrite``
        check that the name still leads to this file.

        *durable* flushes it to stable storage first, for a record that outlives the host that wrote it: a generation a
        crash left empty would wedge every other participant. A holder record dies with its writer, so it skips that.
        """

    def link(self, source: str, target: str) -> bool:
        """Hard-link *source* to *target* without replacing; ``True`` when *target* names *source*'s file afterwards."""

    def replace(self, source: str, target: str) -> bool:
        """Rename *source* over *target* atomically; ``False`` when the platform refused because *target* is in use."""

    def overwrite(self, path: str, data: bytes, identity: tuple[int, int]) -> bool:
        """
        Rewrite from offset zero the file ``create`` returned *identity* for.

        ``False`` when *path* no longer leads to that file.
        """

    def unlink(self, path: str) -> None:
        """Remove the file, tolerating one that is already gone."""

    def listdir(self, path: str) -> list[str]:
        """The names in the directory, or an empty list when it does not exist."""

    def prepare(self, root: str, *, force: bool = False) -> None:
        """
        Create the protocol directories under *root*, refusing anything at those paths that is not a directory.

        Done once per lock; *force* repeats it after a write found a directory missing.
        """


@dataclass(frozen=True)
class Snapshot:
    """The lock's state at one generation: which token writes, which tokens read."""

    generation: int
    writer: str | None
    readers: frozenset[str]

    @property
    def members(self) -> frozenset[str]:
        """Every token this generation names."""
        return self.readers if self.writer is None else self.readers | {self.writer}

    def encode(self) -> bytes:
        lines = [_PROTOCOL, f"generation={self.generation}"]
        if self.writer is not None:
            lines.append(f"writer={self.writer}")
        lines.extend(f"reader={token}" for token in sorted(self.readers))
        return "".join(f"{line}\n" for line in lines).encode("ascii")


def parse_snapshot(data: bytes) -> Snapshot | None:
    """The snapshot a ``gen/<N>`` record encodes, or ``None`` when the record is malformed."""
    if not data or len(data) > _MAX_RECORD_SIZE:
        return None
    try:
        lines = data.decode("ascii").split("\n")
    except UnicodeDecodeError:
        return None
    if lines[0] != _PROTOCOL or lines[-1]:
        return None
    generation: int | None = None
    writer: str | None = None
    readers: set[str] = set()
    for line in lines[1:-1]:
        key, _, value = line.partition("=")
        if (
            key == "generation"
            and generation is None
            and len(value) <= _GENERATION_DIGITS
            and value.isdigit()
            and str(int(value)) == value
        ):
            generation = int(value)
        elif key == "writer" and writer is None and is_token(value):
            writer = value
        elif key == "reader" and is_token(value) and value not in readers:
            readers.add(value)
        else:
            return None
    return None if generation is None else Snapshot(generation=generation, writer=writer, readers=frozenset(readers))


def is_token(value: str) -> bool:
    """Whether *value* is a claim token: exactly 32 lowercase hex digits."""
    return len(value) == _TOKEN_HEX_LENGTH and all(character in "0123456789abcdef" for character in value)


def new_token() -> str:
    """A fresh claim token."""
    return secrets.token_hex(_TOKEN_HEX_LENGTH // 2)


def encode_holder(token: str, nonce: str, lease: float) -> bytes:
    """
    The ``holders/<token>`` record; only the nonce changes between heartbeats, so the length stays constant.

    *lease* is the writer's own ``stale_threshold``: a peer configured tighter must not read this holder's longer
    heartbeat interval as death, so evictors wait the longer of their threshold and this one.
    """
    return (
        f"{_PROTOCOL}\ntoken={token}\npid={os.getpid()}\nhost={host_name()}\nlease={float(lease)!r}\nnonce={nonce}\n"
    ).encode("ascii")


def holder_lease(record: bytes | None) -> float | None:
    """The ``lease`` a holder record carries, or ``None`` when it has none (an older release) or it is malformed."""
    try:
        text = next(
            line for line in (record or b"").decode("ascii").split("\n") if line.startswith("lease=")
        ).removeprefix("lease=")
        lease = float(text)
    except (StopIteration, ValueError):  # UnicodeDecodeError is a ValueError
        return None
    # Only the exact spelling repr() writes: float() alone also takes "1_000", " 1", and non-ASCII digits.
    return lease if isfinite(lease) and lease > 0 and repr(lease) == text else None


class Ledger:
    """
    How long each observed value has stayed the same, measured on this observer's monotonic clock.

    A value that has not changed for ``stale_threshold`` belongs to a participant that stopped refreshing it. The clock
    is the observer's own and only ever measures the gap between two of its own observations, which is what lets two
    hosts agree on liveness without agreeing on the time.
    """

    def __init__(self, clock: Callable[[], float]) -> None:
        self._clock = clock
        self._seen: dict[str, tuple[bytes | None, float]] = {}

    def observe(self, key: str, value: bytes | None, *, asked: float | None = None) -> float:
        """
        Record *value* for *key* and return for how long it has stayed the same.

        *asked* is the clock reading taken before the caller fetched *value*. An unchanged value counts as unchanged up
        to *asked*, so a pause between the fetch and this call adds no time.
        """
        now = self._clock()
        if (seen := self._seen.get(key)) is None or seen[0] != value:
            self._seen[key] = (value, now)
            return 0.0
        return (now if asked is None else asked) - seen[1]

    def forget(self, key: str) -> None:
        self._seen.pop(key, None)

    def retain(self, keys: set[str]) -> None:
        """Drop every key not in *keys*: peers that left would otherwise stay in the ledger for the lock's lifetime."""
        self._seen = {key: seen for key, seen in self._seen.items() if key in keys}


class GenerationLog:
    """The ordered snapshots under ``gen/``, found by listing and probing, advanced by linking the next one."""

    def __init__(self, files: Files, lock_file: str, root: str) -> None:
        self._files = files
        self._lock_file = lock_file
        self._directory = Path(root, _GENERATIONS_DIRECTORY)
        self._epoch_path = str(self._directory / _EPOCH_NAME)
        self._head_path = str(self._directory / _HEAD_NAME)
        self._epoch: bytes | None = None
        self._latest: Snapshot | None = None

    def latest(self) -> Snapshot:
        """
        The newest snapshot.

        Starts from the newest readable generation among those ``gen/HEAD``, the listing, or memory name, then probes
        forward by name across the retained window. ``HEAD`` only names a generation, and that generation must still
        read back: a committer stalled before its rename can leave ``HEAD`` on a generation compacted since. A
        generation compacted between the listing and the read, and a hole left by a committer that died before it
        compacted, are covered by the probe. Memory and ``HEAD`` are bound to the log's epoch, so a log wiped and
        restarted is never continued from a generation it no longer holds. Only a log that names generations, now or as
        remembered, of which none can be read is refused, because that is a client so far behind the log that nothing
        it reads can be trusted.
        """
        # When everything the first pass named is gone, peers compacted past it while this client looked, or the log was
        # replaced; one more pass settles whether this client cannot see the head at all.
        if (latest := self._find()) is None and (latest := self._find()) is None:
            raise SoftFileLockProtocolError(self._lock_file, None, "generation log names no readable snapshot")
        self._latest = latest
        return latest

    def _find(self) -> Snapshot | None:
        if (epoch := self._incarnation()) != self._epoch:
            self._epoch, self._latest = epoch, None
        named = set(self._list())
        # Generation 0 is the empty log, which has no record to read back.
        if self._latest is not None and self._latest.generation:
            named.add(self._latest.generation)
        if head := self._head_generation():
            named.add(head)
        for generation in sorted(named, reverse=True):
            if (start := self._read(generation)) is not None:
                # Compaction keeps the surviving generations contiguous, so the first missing name past a readable HEAD
                # generation is the end; probing the whole window on every poll cost 64 failed opens.
                return self._probe(start, patience=1 if generation == head else _RETAINED_GENERATIONS)
        newest = max(named, default=0)
        found = self._probe(Snapshot(generation=newest, writer=None, readers=frozenset()))
        return found if found.generation > newest or not named else None

    def _head_generation(self) -> int | None:
        # HEAD is never flushed, so a crash can leave it unreadable; readers verify what it names anyway, so a torn one
        # only sends them to the window probe.
        if (record := self._head_record()) is None or (snapshot := parse_snapshot(record)) is None:
            return None
        return snapshot.generation

    def _head_record(self) -> bytes | None:
        if (data := self._files.read(self._head_path)) is None:
            return None
        # A HEAD from another epoch landed after its log was removed; a log whose epoch was never written runs without.
        epoch, newline, record = data.partition(b"\n")
        return record if self._valid_epoch() == epoch + newline else None

    def _valid_epoch(self) -> bytes | None:
        epoch = self._epoch
        return epoch if epoch is not None and is_token(epoch.decode("ascii", "replace").removesuffix("\n")) else None

    def _incarnation(self) -> bytes | None:
        if (epoch := self._files.read(self._epoch_path)) is None:
            # Exclusive creation, not a link: reading the log must keep working where os.link does not exist. A peer
            # that reads the file before its token is written takes the empty value for the epoch, and its next commit
            # sees the change and re-reads, so a partial read costs one retry.
            try:
                self._files.create(self._epoch_path, f"{new_token()}\n".encode("ascii"), durable=True)
            except FileExistsError:
                pass
            except FileNotFoundError:
                # No gen/ yet: there is no log to tell apart, and the first participant's prepare() creates it.
                return None
            epoch = self._files.read(self._epoch_path)
        return epoch

    def _list(self) -> list[int]:
        listed = self._listed()
        for generation in listed[:-_RETAINED_GENERATIONS]:
            self._files.unlink(self._path(generation))
        return listed[-_RETAINED_GENERATIONS:]

    def _listed(self) -> list[int]:
        # isdigit() alone admits Unicode digits: superscripts int() rejects, fullwidth ones it parses to another number.
        return sorted(
            int(name)
            for name in self._files.listdir(str(self._directory))
            if len(name) == _GENERATION_DIGITS and name.isascii() and name.isdigit()
        )

    def _probe(self, start: Snapshot, patience: int = _RETAINED_GENERATIONS) -> Snapshot:
        latest = start
        generation = start.generation
        gap = 0
        while gap < patience:
            generation += 1
            if (snapshot := self._read(generation)) is None:
                gap += 1
                continue
            latest = snapshot
            gap = 0
        return latest

    def _read(self, generation: int) -> Snapshot | None:
        if (data := self._files.read(self._path(generation))) is None:
            return None
        if (snapshot := parse_snapshot(data)) is None or snapshot.generation != generation:
            name = self._name(generation)
            raise SoftFileLockProtocolError(self._lock_file, name, "malformed generation record")
        return snapshot

    def commit(self, successor: Snapshot) -> bool:
        """
        Publish *successor* as the next generation; ``False`` when a peer published that generation first.

        The record is complete before it gets its public name, and a no-replace hard link is the compare-and-swap: only
        one link to ``gen/<N+1>`` can ever succeed. The link's return code is not trusted, because an NFS retry can
        report a link that did land as failed; the file's identity after the call is what decides. A log whose epoch
        changed since the snapshot was read is a different log, so the commit is refused as lost and the caller
        re-reads; that narrows a wipe racing this commit to the moment between the epoch check and the link. A slot
        compaction freed is refused the same way, before the link and again after it, since a committer can stall in
        between. After the link a surviving predecessor proves little: it may be a record another stalled committer
        linked into its own freed slot and has yet to remove. A slot comes free once the log is a whole window past it,
        and the head stays, so a record the listing or ``HEAD`` leads by a window is refused as well. A record refused
        after it landed is removed when nothing was built on it. One that peers lead by less than a window stays, or its
        slot would come free early.
        """
        if self._files.read(self._epoch_path) != self._epoch or not self._extends_the_log(successor.generation):
            return False
        if not self._publish(
            self._path(successor.generation), data := successor.encode(), self._files.link, durable=True
        ):
            return False
        lead = max([*self._listed(), self._head_generation() or 0]) - successor.generation
        if lead >= _RETAINED_GENERATIONS or not self._extends_the_log(successor.generation):
            if not 0 < lead < _RETAINED_GENERATIONS:
                self._files.unlink(self._path(successor.generation))
            return False
        self._latest = successor
        # Readers verify the generation HEAD names, so a failed rename costs a probe, not the commit. A late one would
        # move HEAD back past later commits, so HEAD only ever moves forward.
        with suppress(OSError):
            if (
                (epoch := self._valid_epoch()) is not None
                and self._files.read(self._epoch_path) == epoch
                and (self._head_generation() or 0) < successor.generation
            ):
                self._publish(self._head_path, epoch + data, self._files.replace, durable=False)
        if (expired := successor.generation - _RETAINED_GENERATIONS - 1) > 0:
            self._compact_through(expired)
        return True

    def _extends_the_log(self, generation: int) -> bool:
        # Compaction removes oldest first, so a missing predecessor means compaction freed this slot. The first
        # generation has none; a HEAD of this epoch means the log already moved past it.
        if generation > 1:
            return self._files.read(self._path(generation - 1)) is not None
        return self._head_record() is None

    def _publish(self, target: str, data: bytes, place: Callable[[str, str], bool], *, durable: bool) -> bool:
        temporary = str(self._directory / f"{_TEMPORARY_PREFIX}{new_token()}")
        try:
            self._files.create(temporary, data, durable=durable)
        except FileNotFoundError:
            # No gen/: the log was removed, or nobody has prepared it yet. Either way there is nothing to publish into
            # until a participant's prepare() creates it, and the caller re-reads whatever log exists by then.
            return False
        try:
            return place(temporary, target)
        finally:
            self._files.unlink(temporary)

    def _compact_through(self, expired: int) -> None:
        # Oldest first, so survivors stay contiguous: a stalled committer's leftover would otherwise sit below a hole.
        oldest = expired
        while oldest > 1 and self._files.read(self._path(oldest - 1)) is not None:
            oldest -= 1
        for generation in range(oldest, expired + 1):
            self._files.unlink(self._path(generation))

    def _path(self, generation: int) -> str:
        return str(self._directory / self._name(generation))

    @staticmethod
    def _name(generation: int) -> str:
        return f"{generation:0{_GENERATION_DIGITS}d}"


class Participant:
    """
    One acquisition, from publishing a claim through holding the lock to leaving.

    ``advance`` is polled until it reports the lock granted, ``heartbeat`` runs on the holder's schedule, and ``leave``
    commits the participant out. None of them sleep: the caller owns the deadline and the poll interval. The ledger and
    the log outlive one acquisition: how long a peer's record has stayed unchanged is knowledge every attempt against
    the same lock shares, or a caller whose attempts are each shorter than the stale threshold could never evict.
    """

    def __init__(  # ruff:ignore[too-many-arguments]  # one value per protocol input; the lock builds this once per acquisition
        self,
        files: Files,
        root: str,
        mode: Mode,
        *,
        stale_threshold: float,
        clock: Callable[[], float],
        ledger: Ledger,
        log: GenerationLog,
    ) -> None:
        self.token: Final[str] = new_token()
        self.mode: Final[Mode] = mode
        self.generation: int | None = None
        self._files = files
        self._root = root
        self._stale_threshold = stale_threshold
        self._clock = clock
        self._ledger = ledger
        self._log = log
        self._holder = self._holder_path(self.token)
        self._holder_identity: tuple[int, int] | None = None
        self._entered = False
        self._next_sweep = clock()

    def publish(self) -> None:
        """Create the holder record peers watch for liveness; it exists before the token appears in any snapshot."""
        self._files.prepare(self._root)
        self._create_holder()

    def _create_holder(self) -> None:
        try:
            self._holder_identity = self._files.create(self._holder, self._record())
        except FileNotFoundError:
            # Removing the whole log takes the directories with it.
            self._files.prepare(self._root, force=True)
            self._holder_identity = self._files.create(self._holder, self._record())

    def _record(self) -> bytes:
        return encode_holder(self.token, new_token(), self._stale_threshold)

    def advance(self) -> bool:
        """
        Take the next step toward holding the lock; ``True`` once the lock is granted.

        A reader enters as soon as no live writer is named. A writer enters as soon as no live writer is named, which
        blocks every new reader, then waits for the named readers to leave. Stale members are evicted in the same commit
        that admits this participant, and a writer's final admission is itself a commit, so no admission can rest on a
        snapshot a peer is about to supersede.
        """
        # A lost race is not a reason to wait: re-read and try again before handing the poll interval back to the
        # caller.
        for _ in range(4):
            latest = self._log.latest()
            self._keep_claim_fresh(latest)
            self._sweep(latest)
            stale = frozenset(token for token in latest.members - {self.token} if self._is_stale(token))
            writer = None if latest.writer in stale else latest.writer
            readers = latest.readers - stale
            if not self._entered:
                if writer is not None:
                    # A live writer blocks entry in both modes; only an eviction is worth committing.
                    if stale:
                        self._commit(latest, writer, readers, stale)
                    return False
                writer, readers = (self.token, readers) if self.mode == "write" else (None, readers | {self.token})
            granted = self.mode == "read" or not readers
            if self._entered and not stale and not granted:
                return False
            if (generation := self._commit(latest, writer, readers, stale)) is None:
                continue
            self._entered = True
            if granted:
                self.generation = generation
            return granted
        return False

    def _keep_claim_fresh(self, latest: Snapshot) -> None:
        # The snapshot is the truth about membership: a commit the filesystem reported lost may have landed, and a peer
        # may have evicted this participant while it waited to drain, so both directions are taken from it. A wait can
        # outlast the stale threshold, so the nonce changes on every poll; a peer then never reads a patient contender
        # as a corpse. A record that has gone missing was taken by an evictor or a sweeper that did, or by a removal of
        # the whole log.
        self._entered = self.token in latest.members
        if not self._refresh_nonce():
            with suppress(FileExistsError):
                self._create_holder()

    def _sweep(self, latest: Snapshot) -> None:
        # A participant that crashed before its first commit leaves a record no snapshot names, and a commit that
        # crashed between creating and unlinking its temporary file leaves that too. Neither blocks anyone, so they are
        # collected on the same unchanged-for-a-threshold rule as a member, and only every half threshold at most.
        if (now := self._clock()) < self._next_sweep:
            return
        self._next_sweep = now + self._stale_threshold / 2
        watched = set(latest.members)
        holders = Path(self._root, _HOLDERS_DIRECTORY)
        for name in self._files.listdir(str(holders)):
            if name != self.token and name not in latest.members and is_token(name):
                watched.add(key := f"holder:{name}")
                self._collect(key, str(holders / name))
        generations = Path(self._root, _GENERATIONS_DIRECTORY)
        for name in self._files.listdir(str(generations)):
            if name.startswith(_TEMPORARY_PREFIX):
                watched.add(key := f"temporary:{name}")
                self._collect(key, str(generations / name))
        self._ledger.retain(watched)

    def _collect(self, key: str, path: str) -> None:
        if self._unchanged_for_its_lease(key, path):
            self._files.unlink(path)
            self._ledger.forget(key)

    def _commit(
        self, latest: Snapshot, writer: str | None, readers: frozenset[str], stale: frozenset[str]
    ) -> int | None:
        successor = Snapshot(generation=latest.generation + 1, writer=writer, readers=readers)
        if not self._log.commit(successor):
            return None
        for token in stale:
            self._files.unlink(self._holder_path(token))
            self._ledger.forget(token)
        return successor.generation

    def _is_stale(self, token: str) -> bool:
        return self._unchanged_for_its_lease(token, self._holder_path(token))

    def _unchanged_for_its_lease(self, key: str, path: str) -> bool:
        asked = self._clock()
        record = self._files.read(path)
        return self._ledger.observe(key, record, asked=asked) >= max(self._stale_threshold, holder_lease(record) or 0.0)

    def _holder_path(self, token: str) -> str:
        return str(Path(self._root, _HOLDERS_DIRECTORY, token))

    def _refresh_nonce(self) -> bool:
        if (identity := self._holder_identity) is None:
            return False
        return self._files.overwrite(self._holder, self._record(), identity)

    def heartbeat(self) -> tuple[HeartbeatOutcome, OSError | None]:
        """
        Refresh the nonce and confirm the latest snapshot still names this participant.

        ``lost`` means a peer evicted this participant or removed its holder record: the lock is no longer held, and the
        holder must stop using what it protects. ``transient`` is a filesystem error refreshing the nonce, worth
        retrying on the next tick. A failure to read the log or to sweep decides nothing about this holder's liveness,
        so it is not counted against it.
        """
        try:
            if not self._refresh_nonce():
                return "lost", None
        except TreeReplacedError:
            return "lost", None
        except OSError as error:
            return "transient", error
        try:
            latest = self._log.latest()
            self._sweep(latest)
        except (OSError, TreeReplacedError):
            return "ok", None
        return ("ok" if self.token in latest.members else "lost"), None

    def leave(self) -> None:
        """Commit this participant out of the latest snapshot, then remove its holder record."""
        # The record goes even when the log cannot be read: a contender that never entered then leaves nothing behind,
        # and a member still named ages out the same way with or without one.
        # A tree that was swapped out leaves no record this participant can reach, so there is nothing to commit out of.
        with suppress(TreeReplacedError):
            try:
                while self.token in (latest := self._log.latest()).members:
                    successor = Snapshot(
                        generation=latest.generation + 1,
                        writer=None if latest.writer == self.token else latest.writer,
                        readers=latest.readers - {self.token},
                    )
                    if self._log.commit(successor):
                        break
            finally:
                self._entered = False
                self.generation = None
                self._files.unlink(self._holder)


__all__ = [
    "Files",
    "GenerationLog",
    "HeartbeatOutcome",
    "Ledger",
    "Mode",
    "Participant",
    "Snapshot",
    "TreeReplacedError",
    "encode_holder",
    "holder_lease",
    "is_token",
    "new_token",
    "parse_snapshot",
]
