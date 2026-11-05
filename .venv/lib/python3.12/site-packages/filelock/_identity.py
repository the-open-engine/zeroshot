from __future__ import annotations

__lazy_modules__ = ["hashlib", "pathlib", "socket", "typing"]

import hashlib
import os
import socket
import sys
from errno import EPERM, ESRCH
from pathlib import Path
from typing import Final

#: Every marker format in this package caps the hostname at the RFC 1123 limit and reads a longer one as malformed.
_HOST_NAME_LIMIT: Final[int] = 253
#: The bytes those formats carry verbatim: printable non-space ASCII, less the ``?`` reserved for the escape.
_HOST_NAME_VERBATIM: Final[frozenset[int]] = frozenset(range(0x21, 0x7F)) - {ord("?")}
#: The characters at the end of an over-long name that carry a digest of the part cut off.
_HOST_NAME_DIGEST_LEN: Final[int] = 8
#: The kernel ABI fixes the initial PID namespace's inode (``PID_NS_INIT_INO`` in ``linux/nsfs.h``); every namespace
#: created later draws one from ``0xF0000000`` up, so this value names the host's own namespace.
_INITIAL_PID_NAMESPACE: Final[int] = 0xEFFFFFFC
_NAMESPACE_MARK: Final[str] = "?pidns-"


def host_name() -> str:
    """
    The hostname recorded alongside an owner, so a marker written on another machine is never probed here.

    Marker formats here hold the field to printable non-space ASCII, while ``socket.gethostname()`` reports whatever
    the kernel stores: a space, a newline that forges an extra marker line, or a byte no codec encodes, which Python
    hands back as a surrogate. A holder that publishes one raw writes a marker it cannot read back, and loses the lock
    to the first peer that ages it out as malformed. ``?`` is illegal in a hostname, so escaping it along with every
    out-of-grammar byte as ``?<hex>`` leaves a real name untouched and still keeps two hosts apart, which
    :func:`owner_is_stale` relies on to refuse to probe a foreign PID.

    Linux appends the PID namespace, as Mercurial's lock prefix does. Two containers in one pod share the hostname and
    the boot id but not the PID namespace, so a PID recorded in one names another process, or none, in the other. The
    suffix starts with a ``?`` no escape can produce, so ``a`` in a namespace never reads as a host literally named
    ``a?pidns-...``. The initial namespace adds nothing, which keeps a host's name what older releases recorded.

    An over-long name ends in a digest of the part cut to fit the limit. A bare cut gives every name that shares the
    kept prefix one identity, and :func:`owner_is_stale` would then probe a local PID for another host's holder.
    """
    name = (
        "".join(
            chr(byte) if byte in _HOST_NAME_VERBATIM else f"?{byte:02x}"
            for byte in socket.gethostname().encode("utf-8", "surrogateescape")
        )
        + _pid_namespace_suffix()
    )
    if len(name) > _HOST_NAME_LIMIT:
        keep = _HOST_NAME_LIMIT - _HOST_NAME_DIGEST_LEN - 1
        name = f"{name[:keep]}-{hashlib.sha256(name[keep:].encode()).hexdigest()[:_HOST_NAME_DIGEST_LEN]}"
    # An escape is three characters and a kept byte is never '?', so a bare '?' can only mean an empty hostname.
    return name or "?"


def _pid_namespace_suffix() -> str:
    try:
        namespace: Final = Path("/proc/self/ns/pid").stat().st_ino
    except OSError:  # only Linux 3.8+ has the file, and a sandbox may hide it; nothing else tells namespaces apart
        return ""
    return "" if namespace == _INITIAL_PID_NAMESPACE else f"{_NAMESPACE_MARK}{namespace:x}"


def owner_is_stale(pid: int, hostname: str, start_token: int | None) -> bool:
    """
    Whether the recorded owner is provably gone, so reclaiming its marker cannot detach a live holder.

    Fail closed: return ``True`` only when this process can prove the exact recorded owner is dead. A marker from
    another host cannot be probed; a live PID whose start token still matches, or whose token cannot be read, is the
    holder or is indistinguishable from it; a live PID whose start token differs is a recycled PID, so the process that
    wrote the marker is gone. PostgreSQL, Qt ``QLockFile`` and Mercurial all break a stale lock only on proof of death
    and treat an unreadable or foreign owner as still holding.
    """
    if hostname != host_name():
        return False
    if process_alive(pid) and (
        start_token is None or (current := process_start_token(pid)) is None or _same_start(current, start_token)
    ):
        return False
    # Only a bare hostname can be an older release's, written from a container that shares this host's name.
    return _NAMESPACE_MARK in hostname or not _owner_runs_elsewhere(pid, start_token)


def owner_is_current_process(pid: int, hostname: str, start_token: int | None) -> bool:
    """
    Whether the recorded owner is this process, so a marker a recycled PID left behind never reads as ours.

    Fail closed: a recorded start token must match this process's. One that differs was written by an earlier process
    that reused this PID, and when this process cannot read its own token nothing proves the marker is ours. A marker
    without a token falls back to the PID and hostname.
    """
    if pid != os.getpid() or hostname != host_name():
        return False
    return start_token is None or start_token == process_start_token(pid)


def _same_start(current: int, recorded: int) -> bool:
    # A Linux token carries the boot id in its high 64 bits, or 0 there when the process could not read the boot id.
    # systemd's ProcSubset=pid hides /proc/sys from one unit, so one process may lack the boot id while its peer has
    # it. A missing boot id then proves nothing, and only starttime is compared. Other platforms' tokens fit in the
    # low bits.
    return current == recorded or (0 in {current >> 64, recorded >> 64} and current % 2**64 == recorded % 2**64)


if sys.platform == "win32":  # pragma: win32 cover
    import ctypes
    from ctypes import wintypes

    _KERNEL32: Final[ctypes.WinDLL] = ctypes.WinDLL("kernel32", use_last_error=True)
    _KERNEL32.CloseHandle.argtypes = [wintypes.HANDLE]
    _KERNEL32.CloseHandle.restype = wintypes.BOOL
    _KERNEL32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    _KERNEL32.OpenProcess.restype = wintypes.HANDLE
    _KERNEL32.GetProcessTimes.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
        ctypes.POINTER(wintypes.FILETIME),
    ]
    _KERNEL32.GetProcessTimes.restype = wintypes.BOOL
    _KERNEL32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    _KERNEL32.WaitForSingleObject.restype = wintypes.DWORD

    _WIN_SYNCHRONIZE: Final[int] = 0x100000
    _WIN_PROCESS_QUERY_LIMITED_INFORMATION: Final[int] = 0x1000
    _WIN_ERROR_INVALID_PARAMETER: Final[int] = 87
    _WIN_INHERIT_HANDLE: Final[bool] = False
    _WIN_WAIT_OBJECT_0: Final[int] = 0

    def process_alive(pid: int) -> bool:
        """Whether a process with this PID still runs, treating an access denial as proof it does."""
        handle = _KERNEL32.OpenProcess(_WIN_SYNCHRONIZE, _WIN_INHERIT_HANDLE, pid)
        if handle:
            try:
                # Any open handle keeps an exited process openable, but its process object is signaled once it exits.
                return _KERNEL32.WaitForSingleObject(handle, 0) != _WIN_WAIT_OBJECT_0
            finally:
                _KERNEL32.CloseHandle(handle)
        return ctypes.get_last_error() != _WIN_ERROR_INVALID_PARAMETER

    def process_start_token(pid: int) -> int | None:
        """The process creation FILETIME as a 100ns tick count, or ``None`` when it cannot be read."""
        handle = _KERNEL32.OpenProcess(_WIN_PROCESS_QUERY_LIMITED_INFORMATION, _WIN_INHERIT_HANDLE, pid)
        if not handle:
            return None
        creation, exit_time, kernel_time, user_time = (wintypes.FILETIME() for _ in range(4))
        try:
            if not _KERNEL32.GetProcessTimes(
                handle,
                ctypes.byref(creation),
                ctypes.byref(exit_time),
                ctypes.byref(kernel_time),
                ctypes.byref(user_time),
            ):
                return None  # pragma: no cover  # win32 GetProcessTimes failure path; not reproducible on a live handle
        finally:
            _KERNEL32.CloseHandle(handle)
        return (creation.dwHighDateTime << 32) | creation.dwLowDateTime

else:  # pragma: win32 no cover

    def process_alive(pid: int) -> bool:
        """Whether a process with this PID exists, treating an access denial (``EPERM``) as proof it does."""
        try:
            os.kill(pid, 0)
        except OSError as error:
            if error.errno == ESRCH:
                return False
            if error.errno == EPERM:
                return True
            raise
        return True

    if sys.platform in {"linux", "android"}:  # pragma: linux cover
        # Termux/Android reports sys.platform == "android" but runs the Linux kernel, so /proc/<pid>/stat and the boot
        # id are the same reliable start-time source; treat it exactly like Linux rather than the tokenless fallback.
        # comm (field 2) is wrapped in parentheses and may itself contain spaces or a ')', so the fixed fields start
        # after the final ')'. starttime is field 22 overall, the twentieth of those trailing fields (index 19).
        _STARTTIME_INDEX: Final[int] = 19

        def _read_boot_id() -> int:
            # starttime is measured in clock ticks since boot, so on its own it repeats across a reboot. Folding the
            # boot id into the high bits makes the Linux token reboot-safe like the absolute clocks macOS and Windows
            # expose, while staying a single integer so a 3.29 reader still parses the third marker line. 0 when this
            # process cannot read the boot id leaves the bare starttime, which _same_start compares alone.
            try:
                boot_id = Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii")
                return int(boot_id.strip().replace("-", ""), 16)
            except (OSError, ValueError):
                return 0

        _BOOT_ID: Final[int] = _read_boot_id()
        #: ``TIME_NS_INIT_INO`` in ``linux/proc_ns.h``, fixed by the kernel ABI like the initial PID namespace's inode.
        _INITIAL_TIME_NAMESPACE: Final[int] = 0xEFFFFFFA

        def process_start_token(pid: int) -> int | None:
            """
            The ``/proc/<pid>/stat`` ``starttime`` folded with the boot id.

            ``None`` when the process is gone, or when this process runs in a time namespace with a boottime offset.
            Since Linux 5.6 the kernel shifts ``starttime`` by the reader's offset, so a value read there differs
            from what a peer outside reads.
            """
            return _read_start_token(pid) if _starttime_unshifted() else None

        def _starttime_unshifted() -> bool:
            # setns can move this process into another time namespace, so ask on every read.
            try:
                namespace: Final = Path("/proc/self/ns/time").stat().st_ino
            except FileNotFoundError:  # no entry means no time namespaces on this kernel
                return True
            except OSError:  # any other failure leaves the namespace unknown, so starttime is not comparable
                return False
            if namespace == _INITIAL_TIME_NAMESPACE:
                return True
            # A container runtime may give each container a time namespace without offsets. timens_offsets describes
            # the namespace this process's children get, its own unless it unshared one since.
            try:
                if Path("/proc/self/ns/time_for_children").stat().st_ino != namespace:
                    return False
                offsets: Final = Path("/proc/self/timens_offsets").read_text(encoding="ascii")
            except OSError:
                return False
            # The clock goes by name since Linux 5.8 and by clock id before.
            return any(line.split() in (["boottime", "0", "0"], ["7", "0", "0"]) for line in offsets.splitlines())

        def _read_start_token(pid: int) -> int | None:
            try:
                data = Path(f"/proc/{pid}/stat").read_bytes()
            except OSError:
                return None
            # psutil identifies a process by the same (pid, starttime) pair; the boot id extends that across reboots.
            fields = data[data.rfind(b")") + 1 :].split()
            if len(fields) <= _STARTTIME_INDEX:  # pragma: no cover  # a truncated /proc read, never seen in practice
                return None
            try:
                starttime = int(fields[_STARTTIME_INDEX])
            except ValueError:  # pragma: no cover  # /proc always renders starttime as an integer
                return None
            return (_BOOT_ID << 64) | starttime

        def _owner_runs_elsewhere(pid: int, start_token: int | None) -> bool:
            """
            Whether the owner runs in another PID namespace as *pid*.

            An older release in a container sharing this hostname recorded its in-container PID, which means nothing
            here. Readers without a boottime offset read the same starttime and boot id. A task with the recorded
            start token whose innermost PID is *pid* is therefore the owner; one that started in the same tick is not.
            A reader with an offset cannot compare start tokens, so the innermost PID alone keeps the marker.
            """
            if start_token is None or (_BOOT_ID and start_token >> 64 not in {0, _BOOT_ID}):
                return False
            unshifted: Final = _starttime_unshifted()
            try:
                with os.scandir("/proc") as entries:
                    return any(
                        entry.name.isdigit()
                        and (task := int(entry.name)) != pid
                        and (
                            not unshifted
                            or ((token := _read_start_token(task)) is not None and _same_start(token, start_token))
                        )
                        and _innermost_pid(task) == pid
                        for entry in entries
                    )
            except OSError:
                # A /proc this process cannot list proves nothing either way, which leaves 4.0.12's verdict.
                return False

        def _innermost_pid(task: int) -> int | None:
            try:
                status: Final = Path(f"/proc/{task}/status").read_bytes()
            except OSError:  # pragma: no cover  # the task exited between the stat read and this one
                return None
            # The kernel ends a line only with "\n". A task names itself, and its Name line keeps "\r", "\v" and the
            # other separators that a text-mode read and str.splitlines() split at. A name could then forge a line.
            return next(
                (int(line.split()[-1]) for line in status.split(b"\n") if line.startswith(b"NSpid:")),
                None,  # kernels before 4.1 record no namespace PIDs
            )

    elif sys.platform == "darwin":  # pragma: darwin cover
        import ctypes
        import struct

        _LIBC: Final[ctypes.CDLL] = ctypes.CDLL(None, use_errno=True)
        _CTL_KERN: Final[int] = 1
        _KERN_PROC: Final[int] = 14
        _KERN_PROC_PID: Final[int] = 1
        # kinfo_proc opens with kp_proc.p_starttime (a struct timeval) at offset 0: int64 seconds, int32 microseconds.
        # The offset is fixed by the struct chain kinfo_proc -> extern_proc -> p_un, so a read at 0 is not a guess.
        _TIMEVAL_AT_ZERO: Final[str] = "<qi"
        _TIMEVAL_SIZE: Final[int] = struct.calcsize(_TIMEVAL_AT_ZERO)

        def process_start_token(pid: int) -> int | None:
            """The process start time in microseconds from ``sysctl(KERN_PROC_PID)``, or ``None`` when it is gone."""
            mib = (ctypes.c_int * 4)(_CTL_KERN, _KERN_PROC, _KERN_PROC_PID, pid)
            length = ctypes.c_size_t(0)
            # The size probe reports the kinfo_proc size for any PID, so the read below, not the probe, tells a live
            # process from a gone one: a gone PID leaves the fetch a zero-length success, so re-check the length after.
            if _LIBC.sysctl(mib, 4, None, ctypes.byref(length), None, 0) != 0:
                return None
            buffer = (ctypes.c_char * length.value)()
            if _LIBC.sysctl(mib, 4, buffer, ctypes.byref(length), None, 0) != 0 or length.value < _TIMEVAL_SIZE:
                return None
            seconds, microseconds = struct.unpack_from(_TIMEVAL_AT_ZERO, buffer.raw, 0)
            return seconds * 1_000_000 + microseconds

    else:  # pragma: no cover  # a POSIX platform without a proven start-time source falls back to fail-closed liveness

        def process_start_token(pid: int) -> int | None:
            """No proven start-time source, so the owner carries no token and liveness rests on the PID alone."""
            del pid
            return None


if sys.platform not in {"linux", "android"}:  # pragma: linux no cover  # pragma: android no cover

    def _owner_runs_elsewhere(pid: int, start_token: int | None) -> bool:
        """Without Linux PID namespaces, a PID that is gone or recycled leaves its owner nowhere else to run."""
        del pid, start_token
        return False


__all__ = [
    "host_name",
    "owner_is_current_process",
    "owner_is_stale",
    "process_alive",
    "process_start_token",
]
