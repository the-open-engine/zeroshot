use std::fs::File;
use std::io::Read;
use std::process::{Command, Stdio};

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

async fn blocked_child(containment: Option<ProcessContainment>) -> tokio::process::Child {
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args(["-c", "read ignored"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(containment) = containment {
        configure_process(&mut command, containment);
    }
    command.spawn().assert_value()
}

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

#[test]
fn proc_parsers_and_scans_preserve_kernel_identity_without_mutating_processes() {
    assert_eq!(linux_process_group("123 (worker) S 10 20 30 40"), Some(20));
    assert_eq!(
        linux_process_group("123 (worker ) with spaces) S 10 21 30"),
        Some(21)
    );
    for malformed in ["", "123 worker S 10 20", "123 (worker) S parent group"] {
        assert_eq!(linux_process_group(malformed), None);
    }
    assert_eq!(linux_effective_uid("Uid:\t10 20 30 40\n"), Some(20));
    assert_eq!(linux_effective_uid("Uid:\t10 invalid 30 40\n"), None);

    // SAFETY: these calls only inspect the current process identity and process group.
    let (uid, process_group) = unsafe { (libc::geteuid(), libc::getpgrp()) };
    assert!(
        process_group_has_live_members(process_group).assert_value(),
        "the test process must appear in its own process group scan"
    );
    assert!(
        worker_has_live_members(WorkerMembership::Uid(uid)).assert_value(),
        "the test process must appear in its effective UID scan"
    );
    assert!(validate_linux_worker_boundary(WorkerMembership::Uid(uid)).is_err());

    let absent = i32::MAX;
    assert!(!process_group_has_live_members(absent).assert_value());
    reap_process_group_children(absent).assert_value();
    reap_linux_child(absent).assert_value();
    assert!(zero_result(0).is_ok());
    assert!(zero_result(1).is_err());
    assert!(boolean_result(false).is_ok());
    assert!(boolean_result(true).is_err());
}

#[test]
fn proc_entry_parsing_distinguishes_non_processes_from_corrupt_process_metadata() {
    let directory = tempfile::tempdir().assert_value();
    for (name, stat) in [
        ("123", "123 (valid worker) S 1 77 3"),
        ("456", "malformed stat"),
        ("not-a-pid", "ignored"),
    ] {
        let process = directory.path().join(name);
        std::fs::create_dir(&process).assert_value();
        std::fs::write(process.join("stat"), stat).assert_value();
    }
    let entries = std::fs::read_dir(directory.path())
        .assert_value()
        .map(|entry| {
            let entry = entry.assert_value();
            let name = entry.file_name().to_string_lossy().into_owned();
            (
                name,
                (linux_entry_pid(&entry), linux_entry_process_group(entry)),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_eq!(entries["123"].0, Some(123));
    assert_eq!(entries["123"].1.as_ref().assert_value(), &Some(77));
    assert_eq!(entries["456"].0, Some(456));
    assert_eq!(
        entries["456"].1.as_ref().err().assert_value().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(entries["not-a-pid"].0, None);
    assert_eq!(entries["not-a-pid"].1.as_ref().assert_value(), &None);
}

#[test]
fn wait_and_identity_decisions_cover_kernel_success_retry_absence_and_failure() {
    for (target, result, error, expected) in [
        (ReapTarget::ProcessGroup, 12, None, ReapAction::Retry),
        (ReapTarget::ProcessGroup, 0, None, ReapAction::Complete),
        (
            ReapTarget::ProcessGroup,
            -1,
            Some(libc::ECHILD),
            ReapAction::Complete,
        ),
        (
            ReapTarget::ProcessGroup,
            -1,
            Some(libc::EINTR),
            ReapAction::Retry,
        ),
        (
            ReapTarget::ProcessGroup,
            -1,
            Some(libc::EIO),
            ReapAction::Fail,
        ),
        (ReapTarget::Child, 12, None, ReapAction::Complete),
        (ReapTarget::Child, 0, None, ReapAction::Complete),
        (
            ReapTarget::Child,
            -1,
            Some(libc::ESRCH),
            ReapAction::Complete,
        ),
        (ReapTarget::Child, -1, Some(libc::EIO), ReapAction::Fail),
        (ReapTarget::Child, -1, None, ReapAction::Fail),
    ] {
        assert_eq!(reap_action(target, result, error), expected);
    }

    let mut group_results = std::collections::VecDeque::from([
        Ok(12),
        Err(io::Error::from_raw_os_error(libc::EINTR)),
        Ok(0),
    ]);
    reap_with(ReapTarget::ProcessGroup, || {
        group_results.pop_front().assert_value()
    })
    .assert_value();
    assert!(group_results.is_empty());
    let error = reap_with(ReapTarget::Child, || {
        Err(io::Error::from_raw_os_error(libc::EIO))
    })
    .err()
    .assert_value();
    assert_eq!(error.raw_os_error(), Some(libc::EIO));

    assert!(process_is_missing(&io::Error::from_raw_os_error(
        libc::ESRCH
    )));
    assert!(!process_is_missing(&io::Error::from_raw_os_error(
        libc::EPERM
    )));
    assert!(validate_worker_membership(1).is_ok());
    assert!(validate_worker_membership(0).is_err());
    assert!(validate_worker_membership(u32::MAX).is_err());

    assert!(linux_identity_matches(
        (10, 20, None),
        (10, 10, 20, 20),
        (0, 0)
    ));
    assert!(linux_identity_matches(
        (10, 20, Some(30)),
        (10, 10, 20, 20),
        (1, 30)
    ));
    for mismatch in [
        ((11, 10, 20, 20), (0, 0)),
        ((10, 11, 20, 20), (0, 0)),
        ((10, 10, 21, 20), (0, 0)),
        ((10, 10, 20, 21), (0, 0)),
        ((10, 10, 20, 20), (1, 30)),
    ] {
        assert!(!linux_identity_matches(
            (10, 20, None),
            mismatch.0,
            mismatch.1
        ));
    }
    assert!(!linux_identity_matches(
        (10, 20, Some(30)),
        (10, 10, 20, 20),
        (1, 31)
    ));
}

#[test]
fn absent_worker_membership_cleanup_is_a_noop_with_positive_evidence() {
    let absent = WorkerMembership::Uid(u32::MAX);
    assert!(!worker_has_live_members(absent).assert_value());
    assert!(!reap_and_kill_worker_processes(absent).assert_value());
    kill_linux_worker_processes(absent).assert_value();
}

#[test]
fn coverage_contract_linux_security_reducers_distinguish_authority_occupancy_and_kernel_state() {
    validate_linux_supervisor(0).assert_value();
    assert_eq!(
        validate_linux_supervisor(1).assert_error().kind(),
        io::ErrorKind::PermissionDenied
    );
    validate_linux_worker_availability(false).assert_value();
    assert_eq!(
        validate_linux_worker_availability(true)
            .assert_error()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    validate_linux_subreaper(1).assert_value();
    for state in [0, 2, -1] {
        assert_eq!(
            validate_linux_subreaper(state).assert_error().kind(),
            io::ErrorKind::Other
        );
    }
    validate_linux_identity_match(true).assert_value();
    assert_eq!(
        validate_linux_identity_match(false).assert_error().kind(),
        io::ErrorKind::Other
    );
}

#[test]
fn coverage_contract_worker_security_steps_are_ordered_and_fail_closed_before_exec() {
    use std::cell::RefCell;

    struct SecurityProbe {
        fail_at: Option<&'static str>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl SecurityProbe {
        fn step(&self, name: &'static str) -> Result<(), io::Error> {
            self.calls.borrow_mut().push(name);
            if self.fail_at == Some(name) {
                Err(io::Error::from_raw_os_error(libc::EPERM))
            } else {
                Ok(())
            }
        }
    }

    impl LinuxWorkerSecurity for SecurityProbe {
        fn close_control_descriptors(&self) -> Result<(), io::Error> {
            self.step("close")
        }

        fn drop_identity(
            &self,
            _uid: u32,
            _gid: u32,
            _group: Option<u32>,
        ) -> Result<(), io::Error> {
            self.step("identity")
        }

        fn clear_privileges(&self) -> Result<(), io::Error> {
            self.step("privileges")
        }

        fn verify_identity(
            &self,
            _uid: u32,
            _gid: u32,
            _group: Option<u32>,
        ) -> Result<(), io::Error> {
            self.step("verify")
        }
    }

    let expected = ["close", "identity", "privileges", "verify"];
    for failure in [
        None,
        Some("close"),
        Some("identity"),
        Some("privileges"),
        Some("verify"),
    ] {
        let probe = SecurityProbe {
            fail_at: failure,
            calls: RefCell::new(Vec::new()),
        };
        let result = configure_linux_worker_with(&probe, 10_002, 10_002, Some(20_001));
        let count = failure
            .and_then(|failed| expected.iter().position(|step| *step == failed))
            .map_or(expected.len(), |index| index + 1);
        assert_eq!(probe.calls.into_inner(), expected[..count]);
        assert_eq!(result.is_err(), failure.is_some());
    }
}

#[test]
fn coverage_contract_worker_signal_batch_ignores_disappearance_but_stops_on_real_refusal() {
    let mut visited = Vec::new();
    kill_linux_pids(&[11, 12, 13], |pid| {
        visited.push(pid);
        if pid == 12 {
            Err(io::Error::from_raw_os_error(libc::ESRCH))
        } else {
            Ok(())
        }
    })
    .assert_value();
    assert_eq!(visited, [11, 12, 13]);

    let mut visited = Vec::new();
    let error = kill_linux_pids(&[21, 22, 23], |pid| {
        visited.push(pid);
        if pid == 22 {
            Err(io::Error::from_raw_os_error(libc::EPERM))
        } else {
            Ok(())
        }
    })
    .assert_error();
    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    assert_eq!(visited, [21, 22]);

    assert_eq!(
        kernel_kill(i32::MAX).assert_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert_eq!(
        kill_process_group(i32::MAX).assert_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    let absent = WorkerMembership::Uid(u32::MAX);
    assert!(!reap_and_kill_worker_processes(absent).assert_value());
    kill_linux_worker_processes(absent).assert_value();
}

#[test]
fn final_contract_cleanup_boundary_reports_each_authority_failure_and_only_falls_back_when_required()
 {
    use std::cell::Cell;

    let root_calls = Cell::new(0);
    let errors = kill_process_tree_with(
        Some(77),
        Err(io::Error::from_raw_os_error(libc::EIO)),
        |group| {
            assert_eq!(group, 77);
            Err(io::Error::from_raw_os_error(libc::ESRCH))
        },
        || {
            root_calls.set(root_calls.get() + 1);
            Ok(())
        },
    );
    assert_eq!(root_calls.get(), 0, "an absent group needs no fallback");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("worker process termination failed"));

    let root_calls = Cell::new(0);
    let errors = kill_process_tree_with(
        Some(88),
        Ok(()),
        |_| Err(io::Error::from_raw_os_error(libc::EPERM)),
        || {
            root_calls.set(root_calls.get() + 1);
            Err(io::Error::from_raw_os_error(libc::EIO))
        },
    );
    assert_eq!(root_calls.get(), 1);
    assert_eq!(errors.len(), 2);
    assert!(errors[0].contains("process group termination failed"));
    assert!(errors[1].contains("root process termination fallback failed"));

    let errors = kill_process_tree_with(
        None,
        Ok(()),
        |_| panic!("a missing group must not be signalled"),
        || Err(io::Error::from_raw_os_error(libc::EACCES)),
    );
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("root process termination failed"));
}

#[test]
fn final_contract_worker_boundary_validation_short_circuits_and_preserves_occupancy_errors() {
    use std::cell::Cell;

    for membership in [
        WorkerMembership::Uid(10_001),
        WorkerMembership::SupplementaryGroup(20_001),
    ] {
        let inspected = Cell::new(false);
        assert_eq!(
            validate_linux_worker_boundary_with(1, membership, |_| {
                inspected.set(true);
                Ok(false)
            })
            .assert_error()
            .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(!inspected.get());

        validate_linux_worker_boundary_with(0, membership, |observed| {
            assert_eq!(observed, membership);
            Ok(false)
        })
        .assert_value();
        assert_eq!(
            validate_linux_worker_boundary_with(0, membership, |_| Ok(true))
                .assert_error()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            validate_linux_worker_boundary_with(0, membership, |_| {
                Err(io::Error::from_raw_os_error(libc::EIO))
            })
            .assert_error()
            .raw_os_error(),
            Some(libc::EIO)
        );
    }

    let inspected = Cell::new(false);
    assert_eq!(
        validate_linux_worker_boundary_with(0, WorkerMembership::Uid(0), |_| {
            inspected.set(true);
            Ok(false)
        })
        .assert_error()
        .kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(!inspected.get());
}

#[test]
fn final_contract_worker_cleanup_kills_before_reaping_and_propagates_the_first_unconfirmed_step() {
    let mut killed = Vec::new();
    let mut reaped = Vec::new();
    assert!(
        kill_and_reap_linux_pids_with(
            &[11, 12],
            |pid| {
                killed.push(pid);
                Ok(())
            },
            |pid| {
                reaped.push(pid);
                Ok(())
            },
        )
        .assert_value()
    );
    assert_eq!(killed, [11, 12]);
    assert_eq!(reaped, [11, 12]);

    let mut reaped = Vec::new();
    let error = kill_and_reap_linux_pids_with(
        &[21, 22, 23],
        |_| Ok(()),
        |pid| {
            reaped.push(pid);
            if pid == 22 {
                Err(io::Error::from_raw_os_error(libc::ECHILD))
            } else {
                Ok(())
            }
        },
    )
    .assert_error();
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    assert_eq!(reaped, [21, 22]);

    let mut reaped = false;
    assert_eq!(
        kill_and_reap_linux_pids_with(
            &[31],
            |_| Err(io::Error::from_raw_os_error(libc::EPERM)),
            |_| {
                reaped = true;
                Ok(())
            },
        )
        .assert_error()
        .raw_os_error(),
        Some(libc::EPERM)
    );
    assert!(!reaped);
    assert!(!kill_and_reap_linux_pids_with(&[], |_| Ok(()), |_| Ok(())).assert_value());
}

#[test]
fn final_contract_process_status_classifies_match_disappearance_corruption_and_io_failure() {
    let membership = WorkerMembership::Uid(42);
    assert_eq!(
        linux_process_membership(7, Ok("Uid:\t1 42 3 4\n".to_owned()), membership).assert_value(),
        Some(true)
    );
    assert_eq!(
        linux_process_membership(7, Ok("Uid:\t1 41 3 4\n".to_owned()), membership).assert_value(),
        Some(false)
    );
    assert_eq!(
        linux_process_membership(
            7,
            Err(io::Error::from_raw_os_error(libc::ENOENT)),
            membership,
        )
        .assert_value(),
        None
    );
    assert_eq!(
        linux_process_membership(7, Ok("Uid:\tinvalid\n".to_owned()), membership)
            .assert_error()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        linux_process_membership(7, Err(io::Error::from_raw_os_error(libc::EIO)), membership,)
            .assert_error()
            .raw_os_error(),
        Some(libc::EIO)
    );
}

#[test]
fn coverage_contract_kernel_identity_verification_is_read_only_and_rejects_mismatches() {
    assert_eq!(
        verify_linux_identity(u32::MAX, u32::MAX, None)
            .assert_error()
            .kind(),
        io::ErrorKind::Other
    );
    assert_eq!(
        KernelWorkerSecurity
            .verify_identity(u32::MAX, u32::MAX, Some(u32::MAX))
            .assert_error()
            .kind(),
        io::ErrorKind::Other
    );

    // Exercise every identity comparison against the actual process without mutating it. A normal
    // development shell has several supplementary groups, while the worker contract permits zero
    // or one; either state has a deterministic expected result.
    let mut only_group = 0_u32;
    // SAFETY: these calls only inspect the test process credentials and write one valid u32 slot.
    let (real_uid, uid, real_gid, gid, group_count) = unsafe {
        (
            libc::getuid(),
            libc::geteuid(),
            libc::getgid(),
            libc::getegid(),
            libc::getgroups(0, std::ptr::null_mut()),
        )
    };
    assert!(group_count >= 0);
    let uniform_identity = real_uid == uid && real_gid == gid;
    assert_eq!(
        verify_linux_identity(uid, gid, None).is_ok(),
        uniform_identity && group_count == 0
    );
    let expected_single_group = if group_count == 1 {
        // SAFETY: group_count proved that the one-element output buffer is sufficient.
        assert_eq!(unsafe { libc::getgroups(1, &mut only_group) }, 1);
        true
    } else {
        false
    };
    assert_eq!(
        verify_linux_identity(uid, gid, Some(only_group)).is_ok(),
        uniform_identity && expected_single_group
    );

    // The ordinary unprivileged CI identity can safely exercise the real fail-closed syscall
    // boundary: setgroups is rejected before any credential changes. Clearing an already-empty
    // capability set and enabling no-new-privileges affect only this finished test thread.
    if uid != 0 {
        assert_eq!(
            KernelWorkerSecurity
                .drop_identity(u32::MAX, u32::MAX, None)
                .assert_error()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        KernelWorkerSecurity.clear_privileges().assert_value();
    }
}

#[tokio::test]
async fn coverage_contract_kernel_kill_terminates_only_the_selected_child() {
    let mut child = blocked_child(None).await;
    let pid = i32::try_from(child.id().assert_value()).assert_value();

    kernel_kill(pid).assert_value();
    let status = child.wait().await.assert_value();
    assert!(!status.success());
}

#[tokio::test]
async fn coverage_contract_worker_pre_exec_drops_identity_or_fails_closed_without_authority() {
    const WORKER_ID: u32 = 65_534;
    let mut command = tokio::process::Command::new("/usr/bin/id");
    command
        .arg("-u")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    configure_process(
        &mut command,
        ProcessContainment::WorkerUid {
            uid: WORKER_ID,
            gid: WORKER_ID,
        },
    );

    match command.output().await {
        Ok(output) => {
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).assert_value().trim(),
                "65534"
            );
        }
        Err(error) => assert_eq!(error.raw_os_error(), Some(libc::EPERM)),
    }
}

#[tokio::test]
async fn coverage_contract_process_group_configuration_and_fallback_cleanup_reap_immediate_children()
 {
    let mut grouped = blocked_child(Some(ProcessContainment::ProcessGroup)).await;
    let pid = i32::try_from(grouped.id().assert_value()).assert_value();
    // SAFETY: getpgid only inspects the live child created above.
    assert_eq!(unsafe { libc::getpgid(pid) }, pid);
    assert!(
        kill_process_tree(Some(pid), ProcessContainment::ProcessGroup, &mut grouped).is_empty()
    );
    tokio::time::timeout(std::time::Duration::from_millis(250), grouped.wait())
        .await
        .assert_value()
        .assert_value();

    let mut fallback = blocked_child(Some(ProcessContainment::ProcessGroup)).await;
    assert!(kill_process_tree(None, ProcessContainment::ProcessGroup, &mut fallback).is_empty());
    tokio::time::timeout(std::time::Duration::from_millis(250), fallback.wait())
        .await
        .assert_value()
        .assert_value();
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
