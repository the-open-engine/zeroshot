use std::fs::File;
use std::io::Read;
use std::process::{Command, Stdio};

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn process_files_opened_before_exit_are_treated_as_absent() {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "read line"])
        .stdin(Stdio::piped())
        .spawn()
        .assert_value();
    let files = ["stat", "status"]
        .map(|name| File::open(format!("/proc/{}/{name}", child.id())).assert_value());
    drop(child.stdin.take());
    child.wait().assert_value();

    for mut file in files {
        let mut contents = String::new();
        let read = file.read_to_string(&mut contents);
        assert_eq!(
            read.as_ref().err().and_then(io::Error::raw_os_error),
            Some(libc::ESRCH)
        );
        assert!(
            resolve_linux_process_read(read.map(|_| contents))
                .assert_value()
                .is_none()
        );
    }
}

#[test]
fn process_file_reads_preserve_contents_and_unrelated_errors() {
    assert_eq!(
        resolve_linux_process_read(Ok("process metadata".to_owned())).assert_value(),
        Some("process metadata".to_owned())
    );
    assert!(
        resolve_linux_process_read(Err(io::Error::from_raw_os_error(libc::ENOENT)))
            .assert_value()
            .is_none()
    );
    for code in [libc::EACCES, libc::EPERM, libc::EIO, libc::EINVAL] {
        let error = resolve_linux_process_read(Err(io::Error::from_raw_os_error(code)))
            .err()
            .assert_value();
        assert_eq!(error.raw_os_error(), Some(code));
    }
}
