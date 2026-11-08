//! One private Linux subreaper per local command. The owner socket is a lifetime witness, not
//! authority over arbitrary PIDs. Only kernel-owned children are killed and reaped after root exit
//! or owner loss. The post-fork branch uses only async-signal-safe syscalls and preallocated data.
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::Mutex;
use tokio::process::Command;

#[path = "tree_supervisor/leases.rs"]
mod leases;
pub(crate) use leases::WorkspaceLease;

pub(super) struct Registration {
    owner: UnixStream,
    child: UnixStream,
    leases: Vec<File>,
}
impl Registration {
    pub(super) fn new(workspace: &Path) -> io::Result<Self> {
        let (owner, child) = UnixStream::pair()?;
        // Command installs its stdio after registration, so retained descriptors must never be
        // 0/1/2 even when the embedding application started with standard streams closed.
        let owner = UnixStream::from(retained_descriptor(&owner)?);
        let child = UnixStream::from(retained_descriptor(&child)?);
        owner.set_nonblocking(true)?;
        Ok(Self {
            owner,
            child,
            leases: leases::capture(workspace)?,
        })
    }
    pub(super) fn configure(&self, command: &mut Command) {
        let (owner, child) = (self.owner.as_raw_fd(), self.child.as_raw_fd());
        let mut keep: Vec<_> = self.leases.iter().map(AsRawFd::as_raw_fd).collect();
        keep.push(child);
        keep.sort_unstable();
        // SAFETY: the closure's post-fork supervisor uses only syscalls and captured immutable
        // descriptors/data. The actual command child returns to the standard exec implementation.
        unsafe {
            command.pre_exec(move || enter(owner, child, &keep));
        }
    }
    pub(super) fn capture(self) -> Supervisor {
        Supervisor {
            owner: self.owner,
            status: Mutex::new(None),
        }
    }
}

#[derive(Debug)]
pub(super) struct Supervisor {
    owner: UnixStream,
    status: Mutex<Option<ExitStatus>>,
}
impl Supervisor {
    pub(super) fn stop(&self) -> io::Result<()> {
        if self.status()?.is_some() {
            return Ok(());
        }
        // SAFETY: the private socket is live and the single-byte buffer is readable. NOSIGNAL
        // prevents an already-completed child from delivering SIGPIPE to the embedding process.
        let result = unsafe {
            libc::send(
                self.owner.as_raw_fd(),
                [1_u8].as_ptr().cast(),
                1,
                libc::MSG_NOSIGNAL,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub(super) fn status(&self) -> io::Result<Option<ExitStatus>> {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if status.is_some() {
            return Ok(*status);
        }
        let mut bytes = [0_u8; 4];
        // SAFETY: the owned socket is valid; the fixed buffer is writable. A partial record stays
        // queued until complete, and completion is published only after every child was reaped.
        let count = unsafe {
            libc::recv(
                self.owner.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        if count < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(error)
            };
        }
        if count == 0 {
            return Err(io::Error::other(
                "process supervisor ended without cleanup evidence",
            ));
        }
        if count == bytes.len() as isize {
            *status = Some(ExitStatus::from_raw(i32::from_ne_bytes(bytes)));
        }
        Ok(*status)
    }
}

fn enter(owner: i32, channel: i32, keep: &[i32]) -> io::Result<()> {
    // SAFETY: this runs after fork, before exec; only async-signal-safe syscalls are used.
    let root;
    unsafe {
        libc::close(owner);
        let mut action = std::mem::zeroed::<libc::sigaction>();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        root = libc::fork();
        if root < 0 {
            return Err(io::Error::last_os_error());
        }
        if root == 0 {
            libc::close(channel);
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }
    }
    // The supervisor cannot retain std's exec-error pipe: the actual child's exec owns that
    // handshake. Workspace locks are the only inherited parent files deliberately retained.
    close_other_descriptors(keep);
    supervise(root, channel)
}

fn close_other_descriptors(keep: &[i32]) {
    let mut start = 3_u32;
    for end in keep
        .iter()
        .copied()
        .filter(|fd| *fd >= 3)
        .map(|fd| fd as u32)
        .chain([u32::MAX])
    {
        if start < end {
            close_descriptor_range(start, end);
        }
        start = end.saturating_add(1);
    }
}

fn close_descriptor_range(start: u32, end: u32) {
    // SAFETY: this range excludes stdio, the control socket and selected workspace leases.
    // Old kernels use the same operation one descriptor at a time.
    unsafe {
        if libc::syscall(libc::SYS_close_range, start, end - 1, 0) == 0 {
            return;
        }
        close_descriptors_from_proc(start, end);
    }
}

fn close_descriptors_from_proc(start: u32, end: u32) {
    // SAFETY: getdents64 is a direct syscall, unlike allocator-backed directory iterators.
    // Enumerating open descriptors also covers descriptors above a subsequently lowered rlimit.
    unsafe {
        let directory = libc::open(
            c"/proc/thread-self/fd".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        );
        if directory < 0 {
            libc::_exit(126);
        }
        let mut bytes = [0_u8; 4096];
        loop {
            let count = libc::syscall(
                libc::SYS_getdents64,
                directory,
                bytes.as_mut_ptr(),
                bytes.len(),
            );
            if count == 0 {
                break;
            }
            if count < 0 {
                if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                libc::_exit(126);
            }
            close_descriptor_entries(&bytes[..count as usize], directory, start, end);
        }
        libc::close(directory);
    }
}

fn close_descriptor_entries(bytes: &[u8], directory: i32, start: u32, end: u32) {
    let mut position = 0;
    // Linux getdents64 records have a 19-byte header, followed by a NUL-terminated name.
    while position + 19 <= bytes.len() {
        let record = &bytes[position..];
        let length = usize::from(u16::from_ne_bytes([record[16], record[17]]));
        if length < 20 || length > record.len() {
            // SAFETY: malformed kernel directory data cannot authorize retaining arbitrary FDs.
            unsafe { libc::_exit(126) };
        }
        let mut descriptor = 0_u32;
        let mut numeric = false;
        for byte in record[19..length].iter().take_while(|byte| **byte != 0) {
            if !byte.is_ascii_digit() {
                numeric = false;
                break;
            }
            numeric = true;
            descriptor = descriptor
                .saturating_mul(10)
                .saturating_add(u32::from(*byte - b'0'));
        }
        if numeric && descriptor >= start && descriptor < end && descriptor != directory as u32 {
            // SAFETY: the private helper owns these inherited descriptors, outside its keep set.
            unsafe { libc::close(descriptor as i32) };
        }
        position += length;
    }
}

fn supervise(root: i32, channel: i32) -> ! {
    let mut root_status = None;
    loop {
        let empty = reap_children(root, &mut root_status);
        if root_status.is_some() || owner_stopped(channel) || empty {
            break;
        }
    }
    loop {
        kill_children();
        if reap_children(root, &mut root_status) {
            break;
        }
        pause();
    }
    let bytes = root_status.unwrap_or(127 << 8).to_ne_bytes();
    // SAFETY: all owned descendants are gone. The bounded record contains only wait status,
    // never process output; _exit avoids the inherited Rust runtime and closes retained locks.
    unsafe {
        libc::send(
            channel,
            bytes.as_ptr().cast(),
            bytes.len(),
            libc::MSG_NOSIGNAL,
        );
        libc::_exit(0);
    }
}

fn reap_children(root: i32, root_status: &mut Option<i32>) -> bool {
    loop {
        let mut status = 0;
        // SAFETY: -1 selects only this private subreaper's children. No other thread exists here.
        let child = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if child > 0 {
            if child == root {
                *root_status = Some(status);
            }
        } else if child == 0 {
            return false;
        } else {
            return io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD);
        }
    }
}

fn owner_stopped(channel: i32) -> bool {
    let mut poll = libc::pollfd {
        fd: channel,
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    // SAFETY: poll reads one initialized descriptor. Any owner byte/EOF/error requests cleanup.
    unsafe { libc::poll(&mut poll, 1, 10) > 0 }
}

fn pause() {
    // SAFETY: a descriptor-free poll is an allocation-free short sleep, interrupted safely by signals.
    unsafe {
        libc::poll(std::ptr::null_mut(), 0, 10);
    }
}

fn kill_children() {
    // SAFETY: the path is static. procfs supplies only children of this single-threaded subreaper;
    // no other thread reaps them between enumeration and signaling, so their PIDs cannot be reused.
    unsafe {
        let file = libc::open(
            c"/proc/thread-self/children".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        );
        if file < 0 {
            libc::_exit(126);
        }
        let mut bytes = [0_u8; 4096];
        let mut child = 0_i32;
        loop {
            let count = libc::read(file, bytes.as_mut_ptr().cast(), bytes.len());
            if count <= 0 {
                break;
            }
            for byte in &bytes[..count as usize] {
                if byte.is_ascii_digit() {
                    child = child
                        .saturating_mul(10)
                        .saturating_add(i32::from(*byte - b'0'));
                } else if child > 0 {
                    libc::kill(child, libc::SIGKILL);
                    child = 0;
                }
            }
        }
        if child > 0 {
            libc::kill(child, libc::SIGKILL);
        }
        libc::close(file);
    }
}

fn retained_descriptor(file: &impl AsRawFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl duplicates a borrowed live descriptor, returning unique CLOEXEC ownership.
    let descriptor = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: descriptor was newly allocated above and has no other owning Rust value.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

#[cfg(test)]
#[path = "tree_supervisor/tests.rs"]
mod tests;
