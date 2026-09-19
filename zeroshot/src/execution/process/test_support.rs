//! Thread-local capability fault injection for root containment tests.

#[repr(C)]
struct CapabilityHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CapabilityBits {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

// Linux capabilities belong to the current thread. These tests deliberately use a current-thread
// runtime, remove only CAP_KILL from its effective set, and restore it before test-owned cleanup.
pub(crate) struct KillCapabilityGuard([CapabilityBits; 2]);

impl KillCapabilityGuard {
    pub(crate) fn suspend() -> Self {
        let header = CapabilityHeader {
            version: 0x2008_0522,
            pid: 0,
        };
        let mut original = [CapabilityBits::default(); 2];
        // SAFETY: the version-3 header and two capability entries are valid for these syscalls.
        assert_eq!(
            unsafe { libc::syscall(libc::SYS_capget, &header, original.as_mut_ptr()) },
            0
        );
        let mut reduced = original;
        const CAP_KILL: u32 = 1 << 5;
        assert_ne!(reduced[0].effective & CAP_KILL, 0);
        reduced[0].effective &= !CAP_KILL;
        // SAFETY: only this test thread's effective CAP_KILL is removed; permitted bits are retained.
        assert_eq!(
            unsafe { libc::syscall(libc::SYS_capset, &header, reduced.as_ptr()) },
            0
        );
        Self(original)
    }
}

impl Drop for KillCapabilityGuard {
    fn drop(&mut self) {
        let header = CapabilityHeader {
            version: 0x2008_0522,
            pid: 0,
        };
        // SAFETY: the saved version-3 capability sets remain permitted and apply to this thread.
        assert_eq!(
            unsafe { libc::syscall(libc::SYS_capset, &header, self.0.as_ptr()) },
            0
        );
    }
}
