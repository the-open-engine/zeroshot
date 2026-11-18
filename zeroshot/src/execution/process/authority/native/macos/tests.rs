use std::mem::{MaybeUninit, size_of};

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::execution::process::authority::{Registration, require_human};

#[test]
fn public_process_identity_matches_same_user_bsd_metadata() {
    let pid = std::process::id();
    let identity = process_identity(pid).assert_value();
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let count = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    assert_eq!(count as usize, size_of::<libc::proc_bsdinfo>());
    let info = unsafe { info.assume_init() };
    assert_eq!(identity.pid, info.pbi_pid);
    assert_eq!(identity.parent, info.pbi_ppid);
    assert_eq!(identity.group, info.pbi_pgid);
    assert_eq!(
        identity.born,
        info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec
    );
    assert_eq!(identity.parent, unsafe { libc::getppid() } as u32);
    assert_eq!(identity.group, unsafe { libc::getpgrp() } as u32);
    assert_eq!(process_identity(pid).assert_value(), identity);
}

#[test]
fn protected_init_metadata_supports_registered_human_ancestry() {
    let init = process_identity(1).assert_value();
    assert_eq!(init.pid, 1);
    assert_eq!(init.parent, 0);
    assert_ne!(init.born, 0);
    drop(Registration::prepare().assert_value());
    require_human(std::process::id()).assert_value();
}

#[test]
fn invalid_and_departed_pids_do_not_supply_identity() {
    for pid in [0, u32::MAX] {
        assert_eq!(
            super::super::process_identity(pid).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    assert_eq!(
        process_identity(i32::MAX as u32).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
}

fn time_prefix(seconds: libc::time_t, micros: libc::suseconds_t) -> Vec<u8> {
    let mut bytes = vec![0; size_of::<libc::timeval>()];
    let seconds = seconds.to_ne_bytes();
    bytes[..seconds.len()].copy_from_slice(&seconds);
    let offset = std::mem::offset_of!(libc::timeval, tv_usec);
    let micros = micros.to_ne_bytes();
    bytes[offset..offset + micros.len()].copy_from_slice(&micros);
    bytes
}

#[test]
fn native_time_prefix_preserves_microsecond_creation_identity() {
    assert_eq!(
        parse_birth(&time_prefix(12, 345_678)).assert_value(),
        12_345_678
    );
}

#[test]
fn invalid_native_time_prefixes_never_supply_a_creation_identity() {
    assert_eq!(
        parse_birth(&[0; size_of::<libc::timeval>() - 1])
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    for (seconds, micros) in [(-1, 0), (0, -1), (0, 0), (0, 1_000_000), (i64::MAX, 0)] {
        assert_eq!(
            parse_birth(&time_prefix(seconds, micros))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
