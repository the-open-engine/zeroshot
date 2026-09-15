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

#[test]
fn membership_uses_the_selected_kernel_identity_and_rejects_malformed_groups() {
    let status = "Uid:\t10 20 30 40\nGroups:\t101 202 303 \n";
    for (membership, expected) in [
        (WorkerMembership::Uid(20), true),
        (WorkerMembership::Uid(10), false),
        (WorkerMembership::SupplementaryGroup(202), true),
        (WorkerMembership::SupplementaryGroup(20), false),
    ] {
        assert_eq!(linux_matches_membership(status, membership), Some(expected));
    }
    let group = WorkerMembership::SupplementaryGroup(202);
    assert_eq!(linux_matches_membership("Groups:\t\n", group), Some(false));
    for malformed in [
        "Uid: 1 1 1 1\n",
        "Groups: invalid\n",
        "Groups: 202 invalid\n",
    ] {
        assert_eq!(linux_matches_membership(malformed, group), None);
    }
}

#[tokio::test]
async fn root_writer_membership_survives_namespaces_and_detached_exec() {
    use tokio::io::AsyncBufReadExt;
    use crate::execution::process::{HostedProcessPool, HostedProcessScope};
    use super::super::platform::{
        capture_process_tree, register_process_tree_for, terminate_process_tree,
    };

    let require_namespaces = std::env::var_os("ZEROSHOT_REQUIRE_USER_NAMESPACES").is_some();
    // SAFETY: geteuid only inspects the test process identity.
    if unsafe { libc::geteuid() } != 0 {
        assert!(!require_namespaces, "required namespace probe needs root");
        eprintln!("root-only process membership gate skipped outside the capsule identity");
        return;
    }
    let pool = HostedProcessPool::new(131_002, 131_002, 132_000, 132_000).assert_value();
    let identity = pool
        .identity(HostedProcessScope::WriterExecution(1))
        .assert_value();
    let containment = identity.runner().containment;
    let membership = containment.membership().assert_value();
    let uid_map = format!("0 {} 1\n", identity.uid());
    let gid_map = format!("0 {} 1\n", identity.gid());
    let mut command = tokio::process::Command::new("/bin/sh");
    let script = "/usr/bin/setsid /bin/sleep 30 </dev/null >/dev/null 2>&1 &\n\
        printf '%s %s\n' \"$$\" \"$!\"; read finish";
    command
        .args(["-c", script])
        .current_dir(std::env::temp_dir())
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let registration = register_process_tree_for(containment)
        .unwrap_or_else(|error| panic!("namespace probe registration failed: {error}"));
    configure_process(&mut command, containment);
    // SAFETY: this probe uses only raw syscalls and preallocated mapping bytes after fork.
    unsafe {
        command.pre_exec(move || {
            namespace_group_probe(uid_map.as_bytes(), gid_map.as_bytes(), require_namespaces)
        });
    }
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("namespace probe launch failed: {error}"));
    let handle = capture_process_tree(registration, &mut child).assert_value();
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().assert_value());
    let mut line = String::new();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stdout.read_line(&mut line),
    )
    .await;
    let members = linux_worker_processes(membership);
    let occupied = validate_linux_worker_boundary(membership).is_err();
    let completed = terminate_process_tree(&handle, &mut child).await;
    output
        .unwrap_or_else(|error| panic!("namespace probe output timed out: {error}"))
        .unwrap_or_else(|error| panic!("namespace probe output read failed: {error}"));
    let observed = line
        .split_whitespace()
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("invalid namespace probe PIDs {line:?}: {error}"));
    let members =
        members.unwrap_or_else(|error| panic!("namespace probe membership scan failed: {error}"));
    assert_eq!(observed.len(), 2, "namespace probe output: {line:?}");
    assert!(
        observed.iter().all(|pid| members.contains(pid)),
        "expected {observed:?} in {members:?}"
    );
    assert!(occupied);
    assert!(
        completed.cleanup.proves_tree_empty(),
        "{:?}",
        completed.error
    );
    assert!(!worker_has_live_members(membership).assert_value());
}

fn namespace_group_probe(
    uid_map: &[u8],
    gid_map: &[u8],
    require_namespaces: bool,
) -> io::Result<()> {
    groups_cannot_be_dropped()?;
    // Model the dumpable state after ordinary exec; dropping the UID before exec cleared it.
    // SAFETY: prctl inspects and updates only the post-fork child.
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 1
        || unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 1, 0, 0, 0) } != 0
    {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    if !enter_user_namespace(require_namespaces)? {
        return Ok(());
    }
    map_user_namespace(uid_map, gid_map, require_namespaces)?;
    if enter_user_namespace(require_namespaces)? {
        groups_cannot_be_dropped()?;
    }
    Ok(())
}

fn map_user_namespace(uid_map: &[u8], gid_map: &[u8], require_namespaces: bool) -> io::Result<()> {
    groups_cannot_be_dropped()?;
    for (path, bytes) in [
        (c"/proc/self/uid_map", uid_map),
        (c"/proc/self/setgroups", b"deny\n"),
        (c"/proc/self/gid_map", gid_map),
    ] {
        let result = write_namespace_mapping(path, bytes);
        groups_cannot_be_dropped()?;
        match result {
            Ok(()) => {}
            Err(error)
                if !require_namespaces
                    && matches!(error.raw_os_error(), Some(libc::EPERM | libc::EACCES)) =>
            {
                // Host policies may allow a namespace while denying its UID/GID mappings.
                // The child still proves marker inheritance and cleanup in that namespace.
                report_namespace_probe(b"namespace mapping denied at ", path.to_bytes());
                return Ok(());
            }
            Err(error) => {
                report_namespace_probe(b"namespace mapping failed at ", path.to_bytes());
                return Err(error);
            }
        }
    }
    Ok(())
}

fn groups_cannot_be_dropped() -> io::Result<()> {
    // SAFETY: the empty pointer is valid for a zero-length group list in the post-fork child.
    if unsafe { libc::setgroups(0, std::ptr::null()) } == 0 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EPERM) {
        Ok(())
    } else {
        Err(error)
    }
}

fn enter_user_namespace(require_namespaces: bool) -> io::Result<bool> {
    // SAFETY: this changes only the post-fork child's namespace membership.
    if unsafe { libc::unshare(libc::CLONE_NEWUSER) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if !require_namespaces
        && matches!(
            error.raw_os_error(),
            Some(libc::EPERM | libc::EACCES | libc::EINVAL | libc::ENOSPC)
        )
    {
        report_namespace_probe(
            b"namespace creation unavailable at ",
            b"unshare(CLONE_NEWUSER)",
        );
        return Ok(false);
    }
    report_namespace_probe(b"namespace creation failed at ", b"unshare(CLONE_NEWUSER)");
    Err(error)
}

fn report_namespace_probe(message: &[u8], operation: &[u8]) {
    for bytes in [message, operation, b"\n"] {
        // SAFETY: borrowed bytes stay live; raw write avoids allocation in the post-fork child.
        unsafe {
            libc::write(libc::STDERR_FILENO, bytes.as_ptr().cast(), bytes.len());
        }
    }
}

fn write_namespace_mapping(path: &std::ffi::CStr, bytes: &[u8]) -> io::Result<()> {
    // SAFETY: path is NUL-terminated; bytes remain live; the opened descriptor is closed once.
    unsafe {
        let descriptor = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let written = libc::write(descriptor, bytes.as_ptr().cast(), bytes.len());
        let error = io::Error::last_os_error();
        libc::close(descriptor);
        if written < 0 {
            return Err(error);
        }
        if usize::try_from(written).ok() != Some(bytes.len()) {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
    }
    Ok(())
}
