from __future__ import annotations

__lazy_modules__ = ["filelock._util", "msvcrt", "struct"]

import os
import struct
import sys
import time
from contextlib import contextmanager, suppress
from errno import ENOENT, ENOTDIR, EWOULDBLOCK
from functools import cache
from typing import TYPE_CHECKING, Final, cast

from filelock._api import BaseFileLock
from filelock._util import raise_on_not_writable_file

if TYPE_CHECKING:
    from collections.abc import Generator

if sys.platform == "win32":  # pragma: win32 cover
    import ctypes
    import msvcrt
    from ctypes import wintypes

    _GENERIC_READ: Final[int] = 0x80000000
    _GENERIC_WRITE: Final[int] = 0x40000000
    _SYNCHRONIZE: Final[int] = 0x00100000
    _DELETE: Final[int] = 0x00010000
    _DESIRED_ACCESS: Final[int] = _GENERIC_READ | _GENERIC_WRITE | _SYNCHRONIZE
    _FILE_SHARE_READ_WRITE: Final[int] = (
        0x00000001 | 0x00000002
    )  # read | write; matches os.open (_SH_DENYNO), no delete
    _FILE_OPEN: Final[int] = 1  # open the file only if it exists; the NtCreateFile OPEN_EXISTING
    _FILE_OPEN_IF: Final[int] = 3  # open the file if it exists, create it otherwise; the NtCreateFile OPEN_ALWAYS
    # Attribute-only access skips share-mode checks, so the probe sees a name's deletion state rather than a reader.
    _FILE_READ_ATTRIBUTES: Final[int] = 0x00000080
    _FILE_SHARE_ALL: Final[int] = 0x00000001 | 0x00000002 | 0x00000004  # read | write | delete
    _FILE_ATTRIBUTE_READONLY: Final[int] = 0x00000001
    _FILE_ATTRIBUTE_NORMAL: Final[int] = 0x00000080
    _FILE_ATTRIBUTE_REPARSE_POINT: Final[int] = 0x00000400
    # CreateOptions: keep the handle synchronous (the CRT and msvcrt.locking rely on a maintained file position),
    # refuse a directory, return at once instead of waiting for an oplock holder to answer the break, and open a
    # reparse point rather than following it so the check below acts on the link itself.
    _FILE_SYNCHRONOUS_IO_NONALERT: Final[int] = 0x00000020
    _FILE_NON_DIRECTORY_FILE: Final[int] = 0x00000040
    _FILE_DELETE_ON_CLOSE: Final[int] = 0x00001000
    _FILE_COMPLETE_IF_OPLOCKED: Final[int] = 0x00000100
    _FILE_OPEN_REPARSE_POINT: Final[int] = 0x00200000
    _CREATE_OPTIONS: Final[int] = (
        _FILE_SYNCHRONOUS_IO_NONALERT | _FILE_NON_DIRECTORY_FILE | _FILE_COMPLETE_IF_OPLOCKED | _FILE_OPEN_REPARSE_POINT
    )
    # Listing access takes part in share-mode checks, unlike attribute-only access, so a handle that holds it without
    # FILE_SHARE_DELETE blocks renames and removals of the directory until it closes.
    _FILE_LIST_DIRECTORY: Final[int] = 0x00000001
    _FILE_DIRECTORY_FILE: Final[int] = 0x00000001
    _DIRECTORY_OPTIONS: Final[int] = _FILE_SYNCHRONOUS_IO_NONALERT | _FILE_DIRECTORY_FILE | _FILE_OPEN_REPARSE_POINT
    _OBJ_CASE_INSENSITIVE: Final[int] = 0x00000040  # Win32 name lookups are case-insensitive
    _OWNER_WRITE: Final[int] = 0o200
    _GROUP_AND_OTHERS: Final[int] = 0o077
    # The DACL os.mkdir(mode=0o700) applies, less the inheritance flags a file has no use for: protected, so the
    # directory's inheritable entries stay out, with full access for SYSTEM, Administrators and the owner.
    _OWNER_ONLY_SDDL: Final[str] = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)"
    _SDDL_REVISION_1: Final[int] = 1
    _INVALID_HANDLE_VALUE: Final[int | None] = ctypes.c_void_p(-1).value

    # LockFileEx locks a byte range at an offset carried in OVERLAPPED, independent of the descriptor's file position.
    # msvcrt.locking starts at the current position instead, so a metadata write between lock and unlock could shift
    # the byte a later unlock targets; the explicit offset removes that hazard for both the path lock and #608's
    # descriptor lock.
    _LOCKFILE_FAIL_IMMEDIATELY: Final[int] = 0x00000001
    _LOCKFILE_EXCLUSIVE_LOCK: Final[int] = 0x00000002
    _ERROR_ACCESS_DENIED: Final[int] = 5
    _ERROR_LOCK_VIOLATION: Final[int] = 33  # another handle holds the byte range
    _ERROR_NOT_LOCKED: Final[int] = 158  # the byte range is not locked

    # NtCreateFile returns the raw NTSTATUS as its value, where CreateFileW collapses several of these into one
    # ERROR_ACCESS_DENIED. Telling them apart is the point (#604): a name pending deletion or a share conflict is
    # transient and worth a retry, a real access denial is not.
    _STATUS_SUCCESS: Final[int] = 0x00000000
    # A success code that comes with a handle: another handle's oplock is breaking and its holder has not answered yet.
    _STATUS_OPLOCK_BREAK_IN_PROGRESS: Final[int] = 0x00000108
    _STATUS_ACCESS_DENIED: Final[int] = 0xC0000022
    _STATUS_OBJECT_NAME_NOT_FOUND: Final[int] = 0xC0000034
    _STATUS_OBJECT_PATH_NOT_FOUND: Final[int] = 0xC000003A
    _STATUS_SHARING_VIOLATION: Final[int] = 0xC0000043
    _STATUS_DELETE_PENDING: Final[int] = 0xC0000056
    #: Probes, 2 ms apart, for transient STATUS_ACCESS_DENIED while a peer unlinks the name (#604). A count ensures
    #: every probe runs even when the process stalls; oplock breaks use the caller's wait instead.
    _ACCESS_DENIED_PROBES: Final[int] = 250
    _ACCESS_DENIED_GRACE: Final[float] = 0.5
    _ACCESS_DENIED_RETRY: Final[float] = 0.002

    _ntdll: Final[ctypes.WinDLL] = ctypes.WinDLL("ntdll")
    _kernel32: Final[ctypes.WinDLL] = ctypes.WinDLL("kernel32", use_last_error=True)
    _advapi32: Final[ctypes.WinDLL] = ctypes.WinDLL("advapi32", use_last_error=True)

    class _UNICODE_STRING(ctypes.Structure):  # ruff:ignore[invalid-class-name]  # mirrors the Win32 struct name
        _fields_ = (
            ("Length", wintypes.USHORT),  # byte length, not character count
            ("MaximumLength", wintypes.USHORT),
            ("Buffer", wintypes.LPWSTR),
        )

    class _OBJECT_ATTRIBUTES(ctypes.Structure):  # ruff:ignore[invalid-class-name]  # mirrors the Win32 struct name
        _fields_ = (
            ("Length", wintypes.ULONG),
            ("RootDirectory", wintypes.HANDLE),
            ("ObjectName", ctypes.POINTER(_UNICODE_STRING)),
            ("Attributes", wintypes.ULONG),
            ("SecurityDescriptor", ctypes.c_void_p),
            ("SecurityQualityOfService", ctypes.c_void_p),
        )

    class _IO_STATUS_BLOCK(ctypes.Structure):  # ruff:ignore[invalid-class-name]  # mirrors the Win32 struct name
        _fields_ = (
            ("Status", ctypes.c_void_p),  # a union of NTSTATUS and PVOID, so it is pointer-sized
            ("Information", ctypes.c_void_p),
        )

    class _SECURITY_ATTRIBUTES(ctypes.Structure):  # ruff:ignore[invalid-class-name]  # mirrors the Win32 struct name
        _fields_ = (
            ("nLength", wintypes.DWORD),
            ("lpSecurityDescriptor", ctypes.c_void_p),
            ("bInheritHandle", wintypes.BOOL),
        )

    class _OVERLAPPED(ctypes.Structure):  # mirrors the Win32 struct name
        _fields_ = (
            ("Internal", ctypes.c_void_p),  # ULONG_PTR: pointer-sized, not DWORD, or the x64 layout corrupts Offset
            ("InternalHigh", ctypes.c_void_p),
            ("Offset", wintypes.DWORD),  # the DUMMYUNIONNAME struct, flattened: low 32 bits of the byte offset
            ("OffsetHigh", wintypes.DWORD),
            ("hEvent", wintypes.HANDLE),
        )

    class _BY_HANDLE_FILE_INFORMATION(ctypes.Structure):  # ruff:ignore[invalid-class-name]  # mirrors the Win32 struct name
        _fields_ = (
            ("dwFileAttributes", wintypes.DWORD),
            ("ftCreationTime", wintypes.FILETIME),
            ("ftLastAccessTime", wintypes.FILETIME),
            ("ftLastWriteTime", wintypes.FILETIME),
            ("dwVolumeSerialNumber", wintypes.DWORD),
            ("nFileSizeHigh", wintypes.DWORD),
            ("nFileSizeLow", wintypes.DWORD),
            ("nNumberOfLinks", wintypes.DWORD),
            ("nFileIndexHigh", wintypes.DWORD),
            ("nFileIndexLow", wintypes.DWORD),
        )

    _ntdll.NtCreateFile.restype = wintypes.LONG  # NTSTATUS
    _ntdll.NtCreateFile.argtypes = [
        ctypes.POINTER(wintypes.HANDLE),
        wintypes.DWORD,
        ctypes.POINTER(_OBJECT_ATTRIBUTES),
        ctypes.POINTER(_IO_STATUS_BLOCK),
        ctypes.POINTER(ctypes.c_longlong),  # PLARGE_INTEGER AllocationSize
        wintypes.ULONG,
        wintypes.ULONG,
        wintypes.ULONG,
        wintypes.ULONG,
        ctypes.c_void_p,
        wintypes.ULONG,
    ]
    _ntdll.RtlDosPathNameToNtPathName_U_WithStatus.restype = wintypes.LONG  # NTSTATUS
    _ntdll.RtlDosPathNameToNtPathName_U_WithStatus.argtypes = [
        wintypes.LPCWSTR,
        ctypes.POINTER(_UNICODE_STRING),
        ctypes.c_void_p,
        ctypes.c_void_p,
    ]
    _ntdll.RtlFreeUnicodeString.restype = None
    _ntdll.RtlFreeUnicodeString.argtypes = [ctypes.POINTER(_UNICODE_STRING)]
    _ntdll.RtlNtStatusToDosError.restype = wintypes.ULONG
    _ntdll.RtlNtStatusToDosError.argtypes = [wintypes.LONG]

    _kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    _kernel32.CloseHandle.restype = wintypes.BOOL
    _kernel32.GetFileInformationByHandle.argtypes = [wintypes.HANDLE, ctypes.POINTER(_BY_HANDLE_FILE_INFORMATION)]
    _kernel32.GetFileInformationByHandle.restype = wintypes.BOOL
    _kernel32.LockFileEx.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.POINTER(_OVERLAPPED),
    ]
    _kernel32.LockFileEx.restype = wintypes.BOOL
    _kernel32.UnlockFileEx.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.POINTER(_OVERLAPPED),
    ]
    _kernel32.UnlockFileEx.restype = wintypes.BOOL
    _advapi32.ConvertStringSecurityDescriptorToSecurityDescriptorW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.DWORD,
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(wintypes.ULONG),
    ]
    _advapi32.ConvertStringSecurityDescriptorToSecurityDescriptorW.restype = wintypes.BOOL
    _advapi32.GetNamedSecurityInfoW.argtypes = [
        wintypes.LPCWSTR,
        ctypes.c_int,
        wintypes.DWORD,
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(ctypes.c_void_p),
    ]
    _advapi32.GetNamedSecurityInfoW.restype = wintypes.DWORD
    _advapi32.OpenThreadToken.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.BOOL,
        ctypes.POINTER(wintypes.HANDLE),
    ]
    _advapi32.OpenThreadToken.restype = wintypes.BOOL
    _advapi32.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    _advapi32.OpenProcessToken.restype = wintypes.BOOL
    _advapi32.GetTokenInformation.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
        ctypes.POINTER(wintypes.DWORD),
    ]
    _advapi32.GetTokenInformation.restype = wintypes.BOOL
    _kernel32.GetCurrentProcess.restype = wintypes.HANDLE
    _kernel32.GetCurrentThread.restype = wintypes.HANDLE
    _kernel32.LocalFree.argtypes = [ctypes.c_void_p]
    _kernel32.LocalFree.restype = ctypes.c_void_p
    _kernel32.CreateDirectoryW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(_SECURITY_ATTRIBUTES)]
    _kernel32.CreateDirectoryW.restype = wintypes.BOOL
    _kernel32.CreateFileW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.c_void_p,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.HANDLE,
    ]
    _kernel32.CreateFileW.restype = ctypes.c_void_p

    def _lock_fd_nonblocking(fd: int) -> bool:
        # One nonblocking exclusive LockFileEx attempt shared by WindowsFileLock and lock_descriptor, over the one-byte
        # range at offset 0. True on acquisition, False on contention, raise otherwise. The caller owns fd; the handle
        # from get_osfhandle belongs to the CRT descriptor and must not be closed here.
        overlapped = _OVERLAPPED()  # zero-initialized, so Offset/OffsetHigh/hEvent are 0
        flags = _LOCKFILE_EXCLUSIVE_LOCK | _LOCKFILE_FAIL_IMMEDIATELY
        if _kernel32.LockFileEx(msvcrt.get_osfhandle(fd), flags, 0, 1, 0, ctypes.byref(overlapped)):
            return True
        err = ctypes.get_last_error()
        if err == _ERROR_LOCK_VIOLATION:
            return False
        # A non-contention LockFileEx failure is not reproducible in-process.
        raise ctypes.WinError(err)  # pragma: no cover

    def _unlock_fd(fd: int) -> None:
        overlapped = _OVERLAPPED()  # the same offset 0 and one-byte length the lock used
        if (  # pragma: no cover  # only an interrupted earlier release leaves our own range unlocked
            not _kernel32.UnlockFileEx(msvcrt.get_osfhandle(fd), 0, 1, 0, ctypes.byref(overlapped))
            and (err := ctypes.get_last_error()) != _ERROR_NOT_LOCKED
        ):
            raise ctypes.WinError(err)

    class WindowsFileLock(BaseFileLock):
        """
        Uses ``LockFileEx`` to hard lock a byte range of the lock file on Windows systems.

        Lock file cleanup: Windows attempts to delete the lock file after release, but deletion is
        not guaranteed in multi-threaded scenarios where another thread holds an open handle. The lock
        file may persist on disk, which does not affect lock correctness.
        """

        def _acquire(self) -> None:
            raise_on_not_writable_file(self._lock_path)

            # The reparse test is bound to the opened handle, so a symlink or junction swapped in cannot defeat it
            # through a check-then-open TOCTOU race.
            try:
                fd = _open_non_reparse_fd(self._lock_path, self._open_mode())
            except BlockingIOError:
                return  # an oplock holder has not answered its break; let the retry loop keep its deadline
            if fd is None:
                return  # open contention (share conflict or a name pending deletion); let the retry loop try again
            try:
                locked = _lock_fd_nonblocking(fd)
                if locked:
                    self._mark_descriptor_owned(fd)
            except BaseException:
                # Closing drops the byte-range lock, so a hold already recorded for this descriptor must go with it.
                self._mark_descriptor_released()
                os.close(fd)
                raise
            if not locked:
                os.close(fd)  # another holder owns the byte-range lock; let the retry loop try again

        def _release(self) -> None:
            fd = cast("int", self._context.lock_file_fd)
            # Retain the descriptor until the OS unlock succeeds: if UnlockFileEx raises, the byte-range lock is still
            # held, so is_locked must keep reporting held rather than losing the fd. Only after the unlock commits do
            # close and unlink run as post-unlock cleanup; their failure cannot make the lock held again.
            _unlock_fd(fd)
            self._mark_descriptor_released()
            self._close_released_fd(fd, default_suppresses=False)
            if not self._preserve_lock_file:  # preserve_lock_file keeps a stable file identity for the caller (#605)
                with suppress(OSError):
                    unlink_without_oplock_wait(self._lock_path)

    def _open_non_reparse_fd(path: str, mode: int, *, oplock_wait: float = _ACCESS_DENIED_GRACE) -> int | None:
        """
        Open *path* for locking while refusing reparse points, bound to the handle actually locked.

        The file is opened through ``NtCreateFile`` with ``FILE_OPEN_REPARSE_POINT`` so a symlink or junction planted
        at the path is not followed, and the reparse decision is read from *that* handle via
        ``GetFileInformationByHandle`` rather than from a prior pathname query. Reading the held handle closes the
        check-then-open race: an attacker cannot swap the path between validation and use because both act on the same
        handle. Share mode omits delete so a peer cannot unlink or rename the file out from under a live holder,
        matching ``os.open``'s ``_SH_DENYNO``.

        ``NtCreateFile`` is used instead of ``CreateFileW`` because its return value carries the raw ``NTSTATUS``.
        Windows collapses a transient delete-pending name and a permanent access denial into the same Win32
        ``ERROR_ACCESS_DENIED``; the status keeps them apart, so a real denial fails fast instead of spinning until the
        caller's timeout (#604).

        The reparse option only guards the final path component; Windows still follows reparse points in intermediate
        directories. This assumes the lock file sits in a lock directory untrusted users cannot modify. A path with
        attacker-controlled parent directories would need component-by-component handle validation.

        :param path: the lock file path.
        :param mode: the permission mode; as ``os.open`` does on Windows, a cleared owner-write bit creates the file
            read-only. A mode without group and other bits creates it with an owner-only DACL (discretionary access
            control list, the Windows permission list); any other mode has no Windows counterpart and leaves the file
            the entries its directory hands down. Both only take effect when the file is created, not when an existing
            one is opened.

        :param oplock_wait: seconds to wait for the holder of an oplock on the file to answer its break.

        :returns: a file descriptor owning the opened handle, or ``None`` on a sharing violation or a delete-pending
            name, which the caller should treat as contention and retry.

        :raises BlockingIOError: if an oplock break stays unanswered for *oplock_wait* seconds.
        :raises OSError: if the path resolves to a reparse point, or the open fails for any other reason, raised with
            the Win32 error the status maps to. An access denial is raised only once it outlasts a run of probes, since
            a peer unlinking the file as it releases can make the open answer ``STATUS_ACCESS_DENIED`` for a moment.

        """
        # Emit the audit event os.open would, so consumers watching "open" still see the path-level open and can veto.
        sys.audit("open", path, None, os.O_RDWR | os.O_CREAT)
        read_only = not mode & _OWNER_WRITE
        owner_only = not mode & 0o077
        handle, status = _nt_open(path, read_only=read_only, owner_only=owner_only)
        denied_probes = 0
        oplock_deadline = time.monotonic() + oplock_wait
        while True:
            if status == _STATUS_ACCESS_DENIED:
                if denied_probes >= _ACCESS_DENIED_PROBES:
                    break
                denied_probes += 1
            elif status == _STATUS_OPLOCK_BREAK_IN_PROGRESS:
                if time.monotonic() >= oplock_deadline:
                    break
            else:
                break
            time.sleep(_ACCESS_DENIED_RETRY)
            handle, status = _nt_open(path, read_only=read_only, owner_only=owner_only)
        if status == _STATUS_OPLOCK_BREAK_IN_PROGRESS:
            msg = "oplock break in progress"
            raise BlockingIOError(EWOULDBLOCK, msg, path)
        if status != _STATUS_SUCCESS:
            if status in {_STATUS_SHARING_VIOLATION, _STATUS_DELETE_PENDING}:
                return None
            raise _status_error(status, path)

        return _descriptor_for_non_reparse(handle, path, os.O_RDWR)

    def open_read_without_oplock_wait(path: str) -> int:
        """
        Open *path* read-only and return a descriptor, without waiting for an oplock holder to answer its break.

        ``os.open`` waits inside the system call until the holder of an oplock on the file answers or closes its handle,
        so no timeout or ``blocking=False`` ends the wait. A reparse point at *path* is refused rather than followed.

        :param path: the file to read.

        :returns: a read-only file descriptor.

        :raises BlockingIOError: if an oplock break is in progress, which callers treat as contention.
        :raises FileNotFoundError: if *path* does not exist.
        :raises OSError: if the open fails for any other reason, or *path* is a reparse point.

        """
        sys.audit("open", path, None, os.O_RDONLY)
        return _open_existing_without_oplock_wait(path, _GENERIC_READ | _SYNCHRONIZE, os.O_RDONLY)

    def open_for_delete_without_oplock_wait(path: str) -> int:
        """
        Open *path* with delete access and return a descriptor, without waiting for an oplock holder to answer.

        The open handle shares read, write and delete, so a rename of *path* can proceed while the caller keeps it.
        A holder that took an oplock through another handle is asked to answer by this open, which then raises, so the
        caller skips the rename instead of waiting for it.

        :param path: the file to rename.

        :returns: a file descriptor owning the handle; the caller closes it.

        :raises BlockingIOError: if an oplock break is in progress, which callers treat as contention.
        :raises FileNotFoundError: if *path* does not exist.
        :raises OSError: if the open fails for any other reason, or *path* is a reparse point.

        """
        return _open_existing_without_oplock_wait(path, _DELETE | _FILE_READ_ATTRIBUTES | _SYNCHRONIZE, os.O_RDONLY)

    def unlink_without_oplock_wait(path: str) -> None:
        """
        Delete *path* through a handle that does not wait for an oplock holder to answer its break.

        ``os.unlink`` waits like ``os.open`` does. Here an unanswered break leaves the file in place, which callers
        accept since deleting a lock file is best effort. A symlink at *path* is deleted as a link.

        :param path: the file to delete.

        :raises FileNotFoundError: if *path* does not exist.
        :raises OSError: if the open fails for any other reason, such as a sharing violation.

        """
        sys.audit("os.remove", path, None)
        handle, status = _nt_create(
            path,
            _DELETE | _SYNCHRONIZE,
            _FILE_ATTRIBUTE_NORMAL,
            _FILE_SHARE_ALL,
            _FILE_OPEN,
            options=_CREATE_OPTIONS | _FILE_DELETE_ON_CLOSE,
        )
        if status == _STATUS_SUCCESS:
            _kernel32.CloseHandle(handle)  # deletes the file
        elif status != _STATUS_OPLOCK_BREAK_IN_PROGRESS:
            raise _status_error(status, path)

    def _open_existing_without_oplock_wait(path: str, access: int, flags: int) -> int:
        handle, status = _nt_create(path, access, _FILE_ATTRIBUTE_NORMAL, _FILE_SHARE_ALL, _FILE_OPEN)
        if status != _STATUS_SUCCESS:
            raise _status_error(status, path)
        return _descriptor_for_non_reparse(handle, path, flags)

    def _status_error(status: int, path: str) -> OSError:
        if status == _STATUS_OPLOCK_BREAK_IN_PROGRESS:
            return BlockingIOError(EWOULDBLOCK, "oplock break in progress", path)
        if status in {_STATUS_OBJECT_NAME_NOT_FOUND, _STATUS_OBJECT_PATH_NOT_FOUND}:
            return FileNotFoundError(ENOENT, os.strerror(ENOENT), path)
        winerror = _ntdll.RtlNtStatusToDosError(status)
        return OSError(None, ctypes.FormatError(winerror).strip(), path, winerror)

    def _descriptor_for_non_reparse(handle: int, path: str, flags: int) -> int:
        info = _BY_HANDLE_FILE_INFORMATION()
        # Querying an open handle we just created does not fail.
        if not _kernel32.GetFileInformationByHandle(handle, ctypes.byref(info)):  # pragma: no cover
            err = ctypes.get_last_error()
            _kernel32.CloseHandle(handle)
            raise ctypes.WinError(err)
        if info.dwFileAttributes & _FILE_ATTRIBUTE_REPARSE_POINT:
            _kernel32.CloseHandle(handle)
            msg = f"Lock file is a reparse point (symlink/junction): {path}"
            raise OSError(msg)

        try:
            # O_NOINHERIT mirrors os.open on Windows: the lock fd must not leak into child processes.
            return msvcrt.open_osfhandle(handle, flags | os.O_NOINHERIT)
        except BaseException:  # pragma: no cover  # open_osfhandle audits too; a hook raising must not leak the handle
            _kernel32.CloseHandle(handle)
            raise

    def _open_deletable_for_reading(path: str) -> int:
        """
        Open *path* read-only without blocking a concurrent unlink and without following a reparse point.

        ``os.open`` omits ``FILE_SHARE_DELETE``, so while a process has a file open that way, a delete of it fails with
        a sharing violation. Every contender reads a strict claim and its owner unlinks it, so a reader that keeps the
        claim open past the owner's retries makes the release fail.
        """
        sys.audit("open", path, None, os.O_RDONLY)
        handle, status = _nt_create(path, _GENERIC_READ | _SYNCHRONIZE, 0, _FILE_SHARE_ALL, _FILE_OPEN)
        if status != _STATUS_SUCCESS:
            winerror = _ntdll.RtlNtStatusToDosError(status)
            raise OSError(None, ctypes.FormatError(winerror).strip(), path, winerror)
        try:
            return msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_NOINHERIT)
        except BaseException:  # pragma: no cover  # open_osfhandle audits too; a hook raising must not leak the handle
            _kernel32.CloseHandle(handle)
            raise

    def _pin_directory(path: str) -> int:
        """
        Open the directory at *path* so that no process can rename or remove it until :func:`_unpin_directory`.

        Windows has no ``dir_fd``, so an operation resolves every path component by name again. A handle on each
        directory of a record path, opened without following a reparse point, keeps that name bound to the directory
        checked here until the operation ends.

        :raises NotADirectoryError: when *path* is a junction, a symlink or anything other than a directory.
        """
        sys.audit("open", path, None, os.O_RDONLY)
        # GetFileInformationByHandle needs FILE_READ_ATTRIBUTES; listing access is what takes part in share checks.
        access = _FILE_LIST_DIRECTORY | _FILE_READ_ATTRIBUTES | _SYNCHRONIZE
        handle, status = _nt_create(path, access, 0, _FILE_SHARE_READ_WRITE, _FILE_OPEN, _DIRECTORY_OPTIONS)
        if status != _STATUS_SUCCESS:
            winerror = _ntdll.RtlNtStatusToDosError(status)
            raise OSError(None, ctypes.FormatError(winerror).strip(), path, winerror)
        info = _BY_HANDLE_FILE_INFORMATION()
        # Querying an open handle we just created does not fail.
        if not _kernel32.GetFileInformationByHandle(handle, ctypes.byref(info)):  # pragma: no cover
            error = ctypes.get_last_error()
            _kernel32.CloseHandle(handle)
            raise ctypes.WinError(error)
        if info.dwFileAttributes & _FILE_ATTRIBUTE_REPARSE_POINT:
            _kernel32.CloseHandle(handle)
            raise NotADirectoryError(ENOTDIR, "is a junction or symlink, not a directory", path)
        return handle

    def _unpin_directory(handle: int) -> None:
        _kernel32.CloseHandle(handle)

    def _is_unlinked(path: str) -> bool:
        """
        Report whether *path* is gone or pending deletion, which ``os.open`` reports as the same EACCES as a denial.

        NTFS keeps an unlinked name listed, refusing every open with ``STATUS_DELETE_PENDING``, until the last handle
        to the file closes. Another process holding such a handle, such as an antivirus scan, can keep it that way for
        longer than any retry grace, yet the name's owner has already removed it.
        """
        handle, status = _nt_create(path, _FILE_READ_ATTRIBUTES | _SYNCHRONIZE, 0, _FILE_SHARE_ALL, _FILE_OPEN)
        if handle:
            _kernel32.CloseHandle(handle)
        return status in {_STATUS_OBJECT_NAME_NOT_FOUND, _STATUS_DELETE_PENDING}

    def _nt_open(path: str, *, read_only: bool, owner_only: bool) -> tuple[int, int]:
        """
        Open or create *path* for locking through ``NtCreateFile`` and return ``(handle, status)``.

        Without *owner_only* a file this call creates gets no DACL of its own and inherits its directory's ACL.
        """
        attributes = _FILE_ATTRIBUTE_READONLY if read_only else _FILE_ATTRIBUTE_NORMAL
        descriptor = _owner_only_descriptor() if owner_only else None
        return _nt_create(
            path, _DESIRED_ACCESS, attributes, _FILE_SHARE_READ_WRITE, _FILE_OPEN_IF, descriptor=descriptor
        )

    @cache
    def _owner_only_descriptor() -> ctypes.c_void_p:
        # The DACL os.mkdir(mode=0o700) applies, minus the directory-only inheritance flags: protected (P), so the
        # directory's inheritable entries stay out, with full access (FA) for SYSTEM, Administrators and the owner.
        # Converted once and never freed: the open retries every few milliseconds while it waits.
        descriptor = ctypes.c_void_p()
        if not _advapi32.ConvertStringSecurityDescriptorToSecurityDescriptorW(
            "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)", 1, ctypes.byref(descriptor), None
        ):  # pragma: no cover  # a constant string converts
            raise ctypes.WinError(ctypes.get_last_error())
        return descriptor

    def _nt_create(  # ruff:ignore[too-many-arguments, too-many-positional-arguments]  # NtCreateFile's own knobs
        path: str,
        access: int,
        attributes: int,
        share: int,
        disposition: int,
        options: int = _CREATE_OPTIONS,
        *,
        descriptor: ctypes.c_void_p | None = None,
    ) -> tuple[int, int]:
        """
        Open *path* through ``NtCreateFile`` and return ``(handle, status)``.

        ``RtlDosPathNameToNtPathName_U_WithStatus`` translates the Win32 path to the NT namespace, handling relative,
        drive, UNC and extended-length path forms as Win32 itself would, and allocates a buffer that
        ``RtlFreeUnicodeString`` releases. The handle is ``0`` unless the status is ``STATUS_SUCCESS``: one that comes
        with ``STATUS_OPLOCK_BREAK_IN_PROGRESS`` is closed here, since the file is not ours until the oplock holder
        answers.
        """
        nt_name = _UNICODE_STRING()
        status = _ntdll.RtlDosPathNameToNtPathName_U_WithStatus(path, ctypes.byref(nt_name), None, None) & 0xFFFFFFFF
        if status != _STATUS_SUCCESS:
            return 0, status
        try:
            object_attributes = _OBJECT_ATTRIBUTES()
            object_attributes.Length = ctypes.sizeof(_OBJECT_ATTRIBUTES)
            object_attributes.ObjectName = ctypes.pointer(nt_name)
            object_attributes.Attributes = _OBJ_CASE_INSENSITIVE
            object_attributes.SecurityDescriptor = descriptor
            handle = wintypes.HANDLE()
            io_status = _IO_STATUS_BLOCK()
            status = (
                _ntdll.NtCreateFile(
                    ctypes.byref(handle),
                    access,
                    ctypes.byref(object_attributes),
                    ctypes.byref(io_status),
                    None,
                    attributes,
                    share,
                    disposition,
                    options,
                    None,
                    0,
                )
                & 0xFFFFFFFF
            )
        finally:
            _ntdll.RtlFreeUnicodeString(ctypes.byref(nt_name))
        if status == _STATUS_OPLOCK_BREAK_IN_PROGRESS:
            _kernel32.CloseHandle(handle)
        if status != _STATUS_SUCCESS:
            return 0, status
        return handle.value or 0, status

    def _directory_is_private(path: str) -> bool:
        """
        Report whether only the current user, ``SYSTEM`` and ``Administrators`` can create or remove names in *path*.

        A directory whose owner is anyone else can have its ACL rewritten by that owner. Entries that only a child
        inherits count too, since the DACL says nothing more precise about who may write.
        """
        owner, dacl, descriptor = ctypes.c_void_p(), ctypes.c_void_p(), ctypes.c_void_p()
        error = _advapi32.GetNamedSecurityInfoW(
            path,
            1,  # SE_FILE_OBJECT
            0x1 | 0x4,  # OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION
            ctypes.byref(owner),
            None,
            ctypes.byref(dacl),
            None,
            ctypes.byref(descriptor),
        )
        if error == _ERROR_ACCESS_DENIED:
            # C:\Windows\Temp is one. Anything that cannot be proven private is not.
            return False
        if error:  # pragma: no cover  # the directory was opened or listed an instant earlier
            raise ctypes.WinError(error)
        try:
            # SYSTEM, Administrators, CREATOR OWNER and OWNER RIGHTS; the last two stand for the owner.
            trusted = {_sid_from_text(text) for text in ("S-1-5-18", "S-1-5-32-544", "S-1-3-0", "S-1-3-4")}
            trusted.add(_user_sid())
            # A missing DACL grants everyone full access.
            return (
                bool(dacl.value)
                and _sid_at(cast("int", owner.value)) in trusted
                and _name_writers(cast("int", dacl.value)) <= trusted
            )
        finally:
            _kernel32.LocalFree(descriptor)

    def _name_writers(dacl: int) -> set[bytes]:
        (count,) = struct.unpack("<4xH2x", ctypes.string_at(dacl, 8))
        writers = set()
        offset = 8
        for _ in range(count):
            ace_type, _flags, ace_size, mask = struct.unpack("<BBHI", ctypes.string_at(dacl + offset, 8))
            # Allowed and allowed-callback ACEs share a layout; object ACEs exist only in directory services. The check
            # is conservative: any right that can turn into creating a name counts, namely FILE_ADD_FILE,
            # FILE_ADD_SUBDIRECTORY, FILE_DELETE_CHILD, WRITE_DAC, WRITE_OWNER, GENERIC_WRITE and GENERIC_ALL.
            if ace_type in {0, 9} and mask & (0x2 | 0x4 | 0x40 | 0x40000 | 0x80000 | 0x40000000 | 0x10000000):
                writers.add(_sid_at(dacl + offset + 8))
            offset += ace_size
        return writers

    def _user_sid() -> bytes:
        token = wintypes.HANDLE()
        token_query: Final = 0x0008
        open_as_self: Final = True
        # An impersonating thread acts as the account it impersonates, which is the one whose names the lock creates.
        if not (
            _advapi32.OpenThreadToken(_kernel32.GetCurrentThread(), token_query, open_as_self, ctypes.byref(token))
            or _advapi32.OpenProcessToken(_kernel32.GetCurrentProcess(), token_query, ctypes.byref(token))
        ):  # pragma: no cover  # every process may query its own token
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            size = wintypes.DWORD()
            _advapi32.GetTokenInformation(token, 1, None, 0, ctypes.byref(size))
            buffer = ctypes.create_string_buffer(size.value)
            if not _advapi32.GetTokenInformation(
                token, 1, buffer, size, ctypes.byref(size)
            ):  # pragma: no cover  # the buffer has the size the first call asked for
                raise ctypes.WinError(ctypes.get_last_error())
            return _sid_at(cast("int", ctypes.c_void_p.from_buffer(buffer).value))
        finally:
            _kernel32.CloseHandle(token)

    def _sid_from_text(text: str) -> bytes:
        revision, authority, *subauthorities = (int(part) for part in text.removeprefix("S-").split("-"))
        return (
            struct.pack("<BB", revision, len(subauthorities))
            + authority.to_bytes(6, "big")
            + struct.pack(f"<{len(subauthorities)}I", *subauthorities)
        )

    def _sid_at(address: int) -> bytes:
        return ctypes.string_at(address, 8 + 4 * ctypes.string_at(address + 1, 1)[0])

    def _make_private_directory(path: str) -> None:
        """Create *path* with the owner-only descriptor, so that nobody else can add a name to it."""
        attributes = _SECURITY_ATTRIBUTES(
            nLength=ctypes.sizeof(_SECURITY_ATTRIBUTES),
            lpSecurityDescriptor=_owner_only_descriptor(),
            bInheritHandle=False,
        )
        if not _kernel32.CreateDirectoryW(path, ctypes.byref(attributes)):  # pragma: no cover  # the name is fresh
            raise ctypes.WinError(ctypes.get_last_error())

    @contextmanager
    def _pinned(path: str) -> Generator[tuple[int, int]]:
        """Hold *path* open without delete sharing, which keeps its name from being renamed or removed."""
        with _opened(path, _FILE_SHARE_READ_WRITE) as handle:
            yield _identity_of(handle)

    def _identity_at(path: str) -> tuple[int, int]:
        with _opened(path, _FILE_SHARE_ALL) as handle:
            return _identity_of(handle)

    def _identity_of_fd(fd: int) -> tuple[int, int]:
        return _identity_of(msvcrt.get_osfhandle(fd))

    @contextmanager
    def _opened(path: str, share: int) -> Generator[int]:
        # The final component is opened as itself, never through a reparse point.
        handle = _kernel32.CreateFileW(
            path,
            _FILE_READ_ATTRIBUTES,
            share,
            None,
            3,  # OPEN_EXISTING
            0x02000000 | 0x00200000,  # FILE_FLAG_BACKUP_SEMANTICS opens a directory; FILE_FLAG_OPEN_REPARSE_POINT
            None,
        )
        if handle in {None, _INVALID_HANDLE_VALUE}:  # pragma: no cover  # callers open a name they just made or listed
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            yield cast("int", handle)
        finally:
            _kernel32.CloseHandle(handle)

    def _identity_of(handle: int) -> tuple[int, int]:
        info = _BY_HANDLE_FILE_INFORMATION()
        if not _kernel32.GetFileInformationByHandle(handle, ctypes.byref(info)):  # pragma: no cover
            raise ctypes.WinError(ctypes.get_last_error())
        return info.dwVolumeSerialNumber, info.nFileIndexHigh << 32 | info.nFileIndexLow

else:  # pragma: win32 no cover

    class WindowsFileLock(BaseFileLock):
        """Uses ``LockFileEx`` to hard lock a byte range of the lock file on Windows systems."""

        def _acquire(self) -> None:
            raise NotImplementedError

        def _release(self) -> None:
            raise NotImplementedError


__all__ = [
    "WindowsFileLock",
    "_directory_is_private",
    "_identity_at",
    "_identity_of_fd",
    "_is_unlinked",
    "_make_private_directory",
    "_open_deletable_for_reading",
    "_open_non_reparse_fd",
    "_pin_directory",
    "_pinned",
    "_unpin_directory",
    "open_for_delete_without_oplock_wait",
    "open_read_without_oplock_wait",
    "unlink_without_oplock_wait",
]
