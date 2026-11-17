use super::*;

fn stat(comm: &[u8], parent: &str, group: &str, born: &str) -> Vec<u8> {
    let mut value = b"42 (".to_vec();
    value.extend_from_slice(comm);
    value.extend_from_slice(format!(") S {parent} {group}{} {born}\n", " 0".repeat(16)).as_bytes());
    value
}

#[test]
fn process_stat_extracts_identity_after_final_parenthesis() {
    let value = stat(b"worker (nested) )", "7", "6", "123456789");
    assert_eq!(
        parse_stat(42, &value).unwrap(),
        ProcessIdentity {
            pid: 42,
            parent: 7,
            group: 6,
            born: 123456789
        }
    );
}

#[test]
fn process_stat_ignores_non_utf8_process_names() {
    let value = stat(b"worker\xff)", "7", "6", "123");
    assert_eq!(parse_stat(42, &value).unwrap().born, 123);
}

#[test]
fn process_stat_rejects_a_different_pid() {
    let value = stat(b"worker", "7", "6", "123");
    assert_eq!(
        parse_stat(43, &value).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn process_stat_rejects_invalid_parent_and_group() {
    for invalid in ["-1", "4294967296", "parent"] {
        assert!(parse_stat(42, &stat(b"worker", invalid, "6", "123")).is_err());
        assert!(parse_stat(42, &stat(b"worker", "7", invalid, "123")).is_err());
    }
}

#[test]
fn process_stat_rejects_invalid_creation_identity() {
    for invalid in ["-1", "18446744073709551616", "start"] {
        assert!(parse_stat(42, &stat(b"worker", "7", "6", invalid)).is_err());
    }
}

#[test]
fn process_stat_rejects_truncated_fields_and_missing_delimiters() {
    for value in [
        b"42 (worker) S 7 6".as_slice(),
        b"42 worker S 7 6",
        b"42 (worker S 7 6",
    ] {
        assert_eq!(
            parse_stat(42, value).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn current_process_identity_is_stable_and_reports_native_parent_and_group() {
    let pid = std::process::id();
    let first = process_identity(pid).unwrap();
    let second = process_identity(pid).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.pid, pid);
    assert_eq!(first.parent, unsafe { libc::getppid() } as u32);
    assert_eq!(first.group, unsafe { libc::getpgrp() } as u32);
}
