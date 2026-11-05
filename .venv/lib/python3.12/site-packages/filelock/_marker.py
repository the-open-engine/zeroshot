from __future__ import annotations

__lazy_modules__ = ["contextlib", "filelock._identity", "filelock._util", "math"]

import math
import os
import re
from contextlib import suppress
from typing import Final, Literal, NamedTuple

from filelock._identity import host_name, owner_is_current_process, owner_is_stale, process_start_token
from filelock._soft import SoftFileLock, _read_lock_file, parse_decimal
from filelock._util import pin_inode, write_all

#: Protocol 1 is the legacy ``<pid>\n<hostname>\n[<start_token>\n]`` marker that :class:`SoftFileLock` still writes.
#: Protocol 2 carries the owner mode and the lease claim. A protocol 1 reader treats a protocol 2 marker as malformed
#: and evicts it after its grace period, so the two never guarantee mutual exclusion against each other.
_PROTOCOL: Final[str] = "filelock/2"

_MAX_PID: Final[int] = 2**31 - 1

#: The forms ``repr`` gives a non-negative finite float or int; ``float()`` would also accept whitespace, underscores
#: and non-ASCII digits.
_DURATION: Final[re.Pattern[str]] = re.compile(r"[0-9]+(?:\.[0-9]+)?(?:e[+-][0-9]+)?", re.ASCII)

#: Preserve unknown contracts so contenders cannot mistake them for malformed, reclaimable markers.
OwnerMode = Literal["lease", "exclusive", "unknown"]


class MarkerSoftFileLock(SoftFileLock):
    """An existence lock whose marker carries a protocol 2 owner record."""

    #: Age-based expiry requires a lease contract; an exclusive holder grants none.
    _owner_mode: OwnerMode = "exclusive"

    @property
    def owner(self) -> OwnerRecord | None:
        """
        The owner named by the marker on disk.

        :returns: the published record, or ``None`` when no marker exists or its record is malformed or protocol 1

        """
        return self._read_owner()

    @property
    def pid(self) -> int | None:
        """
        The PID of the process holding this lock, read from the marker.

        :returns: the PID, or ``None`` when no marker exists or its record is unreadable

        """
        return None if (owner := self._read_owner()) is None else owner.pid

    @property
    def is_lock_held_by_us(self) -> bool:
        """
        Whether the marker on disk names this process.

        :returns: ``True`` when the marker names this process's PID, hostname and, when recorded, start token

        """
        owner = self._read_owner()
        return owner is not None and owner_is_current_process(owner.pid, owner.hostname, owner.start)

    def force_break(self) -> None:
        """
        Remove the marker whoever holds it, so a later contender can acquire.

        Forced breaking voids mutual exclusion: the previous holder keeps running and keeps using whatever the lock
        protects. Reserve it for an operator clearing a marker whose holder is known to be gone.
        """
        self.break_lock()

    def _try_break_stale_lock(self) -> None:
        with suppress(OSError, ValueError), pin_inode(self._lock_path) as pinned:
            content, marker = _read_lock_file(self._lock_path)
            owner: Final[OwnerRecord | None]
            if pinned not in {None, marker.st_ino}:  # pragma: win32 no cover  # Windows pins nothing
                return  # replaced before the read, so nothing keeps the number read from reuse
            if (owner := parse_marker(content)) is None:
                super()._try_break_stale_lock()
            elif owner.mode != "unknown" and owner_is_stale(owner.pid, owner.hostname, owner.start):
                self._take_over(marker, owner.pid)

    @classmethod
    def _recorded_holder(cls, content: str | None) -> tuple[int, str] | None:
        if (owner := parse_marker(content)) is not None:
            return owner.pid, owner.hostname
        return super()._recorded_holder(content)

    @classmethod
    def _marker_is_live(cls, content: str | None, breaker_pid: int) -> bool:  # pragma: win32 no cover
        if (owner := parse_marker(content)) is None:
            return super()._marker_is_live(content, breaker_pid)
        return owner.mode == "unknown" or (
            (owner.pid != breaker_pid or owner.hostname != host_name())
            and not owner_is_stale(owner.pid, owner.hostname, owner.start)
        )

    def _read_owner(self) -> OwnerRecord | None:
        with suppress(OSError, ValueError):
            return parse_marker(_read_lock_file(self._lock_path)[0])
        return None

    def _write_lock_info(self, fd: int) -> None:
        write_all(fd, encode_marker(self._published_record()))

    def _published_record(self) -> OwnerRecord:
        return OwnerRecord(
            pid=os.getpid(),
            hostname=host_name(),
            mode=self._owner_mode,
            start=process_start_token(os.getpid()),
        )


class OwnerRecord(NamedTuple):
    """Keep optional metadata when reading unknown lock modes."""

    pid: int
    hostname: str
    mode: OwnerMode
    token: str | None = None
    lease_duration: float | None = None
    start: int | None = None


def encode_marker(record: OwnerRecord) -> bytes:
    """Render an owner record as the bytes a protocol 2 marker holds."""
    lines = [_PROTOCOL, f"pid={record.pid}", f"host={record.hostname}", f"mode={record.mode}"]
    if record.token is not None:
        lines.append(f"token={record.token}")
    if record.lease_duration is not None:
        lines.append(f"duration={record.lease_duration!r}")
    if record.start is not None:
        lines.append(f"start={record.start}")
    return "".join(f"{line}\n" for line in lines).encode()


def parse_marker(content: str | None) -> OwnerRecord | None:
    """Return the owner a protocol 2 marker names, or ``None`` when the record is malformed or protocol 1."""
    # A torn write may end in a prefix like "mode=exclu", an unknown contract nobody reclaims; malformed, it ages out.
    if not content or content[-1] != "\n" or not (lines := content.strip().splitlines()) or lines[0] != _PROTOCOL:
        return None
    fields: dict[str, str] = {}
    for line in lines[1:]:
        key, separator, value = line.partition("=")
        if not separator:
            return None
        fields[key] = value
    return _build_record(fields)


def _build_record(fields: dict[str, str]) -> OwnerRecord | None:
    # An unknown key is a field a newer filelock published, so ignore it rather than read the record as malformed. An
    # unrecognized mode is the same story one level up: a contract this version does not implement. Reading it as
    # malformed would age the marker out of a live owner's hands, so keep it and let the caller refuse to reclaim it.
    # A record naming no mode at all states no contract and stays malformed.
    if (published := fields.get("mode")) is None:
        return None
    mode: Final[OwnerMode] = published if published in {"lease", "exclusive"} else "unknown"
    hostname = fields.get("host")
    if not hostname or "pid" not in fields or ("duration" in fields and not _DURATION.fullmatch(fields["duration"])):
        return None
    try:
        pid = parse_decimal(fields["pid"])
        duration = float(fields["duration"]) if "duration" in fields else None
        start = parse_decimal(fields["start"]) if "start" in fields else None
    except ValueError:
        return None
    if not 1 <= pid <= _MAX_PID:
        return None
    token = fields.get("token")
    # "1e400" passes the grammar but overflows to inf, which mismatches every configured duration and so wedges reclaim,
    # where a malformed marker ages out.
    if mode == "lease" and (not token or duration is None or not (math.isfinite(duration) and duration > 0)):
        return None
    return OwnerRecord(pid=pid, hostname=hostname, mode=mode, token=token, lease_duration=duration, start=start)


__all__ = [
    "MarkerSoftFileLock",
    "OwnerMode",
    "OwnerRecord",
    "encode_marker",
    "parse_marker",
]
