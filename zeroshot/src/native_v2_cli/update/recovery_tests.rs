use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use super::*;

const TARGET_RELEASE: ReleaseVersion = ReleaseVersion([8, 2, 1]);
/// Bounds each wait of the overlap below, so serialized installations proceed after it.
const OVERLAP_WAIT: Duration = Duration::from_secs(5);

#[test]
fn overlapping_installs_succeed_and_keep_the_restic_sidecar_of_the_installed_release() {
    let directory = tempfile::tempdir().unwrap();
    let current = directory
        .path()
        .join(format!("zeroshot{}", std::env::consts::EXE_SUFFIX));
    let restic = directory.path().join("restic");
    fs::write(&current, b"old binary").unwrap();
    fs::write(&restic, b"old restic").unwrap();
    let executables = test_executables();
    let replacements = AtomicUsize::new(0);
    let overtaken_installed_restic = UpdateEvent::default();
    let overtaking_finished = UpdateEvent::default();

    // Two updates of one installation, both started before either replaced it. Unless they are
    // serialized, the overtaken update installs its Restic first, the overtaking update then
    // installs its own and replaces the executable, and only then does the overtaken update try
    // to replace the executable.
    let (overtaken, overtaking) = std::thread::scope(|scope| {
        let overtaken = scope.spawn(|| {
            update_outcome(install_at(
                &current,
                &executables,
                TARGET_RELEASE,
                InstallOperations {
                    verify: verify_release_executable,
                    verify_restic: verify_release_restic,
                    replace: |staged: &Path| {
                        overtaken_installed_restic.signal();
                        overtaking_finished.wait();
                        replace_as_update_started_before_any_replacement(
                            staged,
                            &current,
                            &replacements,
                        )
                    },
                },
            ))
        });
        let overtaking = scope.spawn(|| {
            let outcome = update_outcome(install_at(
                &current,
                &executables,
                TARGET_RELEASE,
                InstallOperations {
                    verify: verify_release_executable,
                    verify_restic: |staged: &Path| {
                        overtaken_installed_restic.wait();
                        verify_release_restic(staged)
                    },
                    replace: |staged: &Path| {
                        replace_as_update_started_before_any_replacement(
                            staged,
                            &current,
                            &replacements,
                        )
                    },
                },
            ));
            overtaking_finished.signal();
            outcome
        });
        (overtaken.join().unwrap(), overtaking.join().unwrap())
    });

    assert_eq!(
        (
            overtaken,
            overtaking,
            read_text(&current),
            read_text(&restic),
            replacements.into_inner(),
            staged_update_files(directory.path()),
        ),
        (
            Ok(()),
            Ok(()),
            "new binary".to_owned(),
            "new restic".to_owned(),
            1,
            Vec::<String>::new(),
        ),
        "(overtaken, overtaking, executable, Restic, replacements, staged files)"
    );
}

#[test]
fn an_install_removes_files_staged_by_an_interrupted_update() {
    let directory = tempfile::tempdir().unwrap();
    let suffix = std::env::consts::EXE_SUFFIX;
    let current = directory.path().join(format!("zeroshot{suffix}"));
    let restic = directory.path().join("restic");
    fs::write(&current, b"old binary").unwrap();
    fs::write(&restic, b"old restic").unwrap();
    // What an update killed an hour ago leaves: its partly staged executables, named as
    // stage_executable names them, and the Restic sidecar it moved aside.
    let interrupted = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
    for (name, contents) in [
        (
            format!(".zeroshot-update-Kq3vZx{suffix}"),
            b"new bin".as_slice(),
        ),
        (
            format!(".restic-update-8YXWgR{suffix}"),
            b"new res".as_slice(),
        ),
        (
            ".restic-update-backup-01a1ab8a-c74d-74f1-b0be-e00fa01e73e5".to_owned(),
            b"old restic".as_slice(),
        ),
    ] {
        let path = directory.path().join(name);
        fs::write(&path, contents).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(interrupted)
            .unwrap();
    }

    let outcome = update_outcome(install_at(
        &current,
        &test_executables(),
        TARGET_RELEASE,
        InstallOperations {
            verify: verify_release_executable,
            verify_restic: verify_release_restic,
            replace: |staged: &Path| {
                fs::copy(staged, &current)?;
                Ok(())
            },
        },
    ));

    assert_eq!(
        (
            outcome,
            read_text(&current),
            read_text(&restic),
            staged_update_files(directory.path()),
        ),
        (
            Ok(()),
            "new binary".to_owned(),
            "new restic".to_owned(),
            Vec::<String>::new(),
        ),
        "(outcome, executable, Restic, staged files)"
    );
}

#[test]
fn a_failed_install_restores_the_restic_sidecar_an_interrupted_update_moved_aside() {
    let directory = tempfile::tempdir().unwrap();
    let current = directory
        .path()
        .join(format!("zeroshot{}", std::env::consts::EXE_SUFFIX));
    fs::write(&current, b"old binary").unwrap();
    // An update killed after moving the installed Restic aside and before installing its own.
    fs::write(
        directory
            .path()
            .join(".restic-update-backup-01a1ab8a-c74d-74f1-b0be-e00fa01e73e5"),
        b"old restic",
    )
    .unwrap();

    let outcome = update_outcome(install_at(
        &current,
        &test_executables(),
        TARGET_RELEASE,
        InstallOperations {
            verify: verify_release_executable,
            verify_restic: |_: &Path| {
                Err(update_error(
                    "downloaded Restic executable failed verification",
                ))
            },
            replace: |_: &Path| Err(update_error("unexpected replacement")),
        },
    ));

    assert_eq!(
        (
            outcome,
            read_text(&current),
            fs::read(directory.path().join("restic"))
                .ok()
                .map(|contents| String::from_utf8_lossy(&contents).into_owned()),
            staged_update_files(directory.path()),
        ),
        (
            Err("update failed: downloaded Restic executable failed verification".to_owned()),
            "old binary".to_owned(),
            Some("old restic".to_owned()),
            Vec::<String>::new(),
        ),
        "(outcome, executable, Restic, staged files)"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn an_update_whose_executable_was_replaced_while_it_waited_finds_the_installed_release() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("zeroshot");
    let restic = directory.path().join("restic");
    // What the update that finished first installed.
    fs::write(&installed, b"new binary").unwrap();
    fs::write(&restic, b"new restic").unwrap();

    let reported = reported_update(install_at(
        // How Linux reports the executable of a process whose executable another update replaced.
        &directory.path().join("zeroshot (deleted)"),
        &test_executables(),
        TARGET_RELEASE,
        InstallOperations {
            verify: |_: &Path, _| Err(update_error("unexpected verification")),
            verify_restic: |_: &Path| Err(update_error("unexpected Restic verification")),
            replace: |_: &Path| Err(update_error("unexpected replacement")),
        },
    ));

    assert_eq!(
        (
            reported,
            read_text(&installed),
            read_text(&restic),
            staged_update_files(directory.path()),
        ),
        (
            Ok(Some(false)),
            "new binary".to_owned(),
            "new restic".to_owned(),
            Vec::<String>::new(),
        ),
        "(reported update, executable, Restic, staged files)"
    );
}

#[test]
fn concurrent_installs_of_one_release_report_one_update() {
    let directory = tempfile::tempdir().unwrap();
    let current = directory
        .path()
        .join(format!("zeroshot{}", std::env::consts::EXE_SUFFIX));
    fs::write(&current, b"old binary").unwrap();
    fs::write(directory.path().join("restic"), b"old restic").unwrap();
    let executables = test_executables();
    let install = || {
        reported_update(install_at(
            &current,
            &executables,
            TARGET_RELEASE,
            InstallOperations {
                verify: verify_release_executable,
                verify_restic: verify_release_restic,
                replace: |staged: &Path| {
                    fs::copy(staged, &current)?;
                    Ok(())
                },
            },
        ))
    };

    let mut reported = std::thread::scope(|scope| {
        let first = scope.spawn(install);
        let second = scope.spawn(install);
        [first.join().unwrap(), second.join().unwrap()]
    });
    reported.sort();

    assert_eq!(
        (reported, read_text(&current)),
        ([Ok(Some(false)), Ok(Some(true))], "new binary".to_owned()),
        "(reported updates, executable)"
    );
}

#[test]
fn an_install_removes_executable_copies_an_interrupted_replacement_left_behind() {
    let directory = tempfile::tempdir().unwrap();
    let current = directory
        .path()
        .join(format!("zeroshot{}", std::env::consts::EXE_SUFFIX));
    fs::write(&current, b"old binary").unwrap();
    fs::write(directory.path().join("restic"), b"old restic").unwrap();
    // self_replace copies the new executable beside the installed one before renaming it over it,
    // under these names on Unix and Windows. Another tool's copy in a shared directory stays.
    let copies = [
        ".zeroshot.__temp__Ab12Cd",
        ".zeroshot.abcdefghijklmnopqrstuvwxyzabcdef.__temp__.exe",
    ];
    let unrelated = ".othertool.__temp__Ef34Gh";
    for name in copies.iter().chain([&unrelated]) {
        fs::write(directory.path().join(name), b"new bin").unwrap();
    }

    let outcome = update_outcome(install_at(
        &current,
        &test_executables(),
        TARGET_RELEASE,
        InstallOperations {
            verify: verify_release_executable,
            verify_restic: verify_release_restic,
            replace: |staged: &Path| {
                fs::copy(staged, &current)?;
                Ok(())
            },
        },
    ));

    assert_eq!(
        (
            outcome,
            read_text(&current),
            copies.map(|name| directory.path().join(name).exists()),
            directory.path().join(unrelated).exists(),
        ),
        (Ok(()), "new binary".to_owned(), [false, false], true),
        "(outcome, executable, copies remaining, unrelated copy remaining)"
    );
}

#[cfg(unix)]
#[test]
fn an_install_shares_an_update_lock_file_it_cannot_write() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let current = directory.path().join("zeroshot");
    fs::write(&current, b"old binary").unwrap();
    fs::write(directory.path().join("restic"), b"old restic").unwrap();
    // The lock file of an installation shared with the user who created it.
    let lock = directory.path().join(".zeroshot-update.lock");
    fs::write(&lock, b"").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o444)).unwrap();

    let outcome = update_outcome(install_at(
        &current,
        &test_executables(),
        TARGET_RELEASE,
        InstallOperations {
            verify: verify_release_executable,
            verify_restic: verify_release_restic,
            replace: |staged: &Path| {
                fs::copy(staged, &current)?;
                Ok(())
            },
        },
    ));

    assert_eq!(
        (outcome, read_text(&current)),
        (Ok(()), "new binary".to_owned()),
        "(outcome, executable)"
    );

    // Without a lock file, an installation the caller cannot write reports why.
    if unsafe { libc::geteuid() } != 0 {
        fs::remove_file(&lock).unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let outcome = update_outcome(install_at(
            &current,
            &test_executables(),
            TARGET_RELEASE,
            InstallOperations {
                verify: verify_release_executable,
                verify_restic: verify_release_restic,
                replace: |_: &Path| Err(update_error("unexpected replacement")),
            },
        ));
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let error = outcome.unwrap_err();
        assert!(error.contains("Permission denied"), "{error}");
    }
}

/// The outcome of one update, whatever a successful installation reports.
fn update_outcome<T>(result: Result<T, NativeV2CliError>) -> Result<(), String> {
    result.map(|_| ()).map_err(|error| error.to_string())
}

/// What one update reports as `updated`, or `None` when its installation reports nothing. Generic
/// like `update_outcome`, so these tests also run against an updater that reports nothing.
fn reported_update<T: Into<serde_json::Value>>(
    result: Result<T, NativeV2CliError>,
) -> Result<Option<bool>, String> {
    result
        .map(|updated| updated.into().as_bool())
        .map_err(|error| error.to_string())
}

/// Fake executables report the target release exactly when they hold its bytes.
fn verify_release_executable(path: &Path, version: ReleaseVersion) -> Result<(), NativeV2CliError> {
    if version == TARGET_RELEASE && fs::read(path).is_ok_and(|contents| contents == b"new binary") {
        Ok(())
    } else {
        Err(update_error(
            "downloaded release executable failed verification",
        ))
    }
}

fn verify_release_restic(path: &Path) -> Result<(), NativeV2CliError> {
    if fs::read(path).is_ok_and(|contents| contents == b"new restic") {
        Ok(())
    } else {
        Err(update_error(
            "downloaded Restic executable failed verification",
        ))
    }
}

/// Replaces the executable as `self_replace` does for an update process that started before any
/// replacement. On Linux, once another update replaced the executable, the process's own path
/// names a deleted file and `self_replace` fails with exactly this error.
fn replace_as_update_started_before_any_replacement(
    staged: &Path,
    current: &Path,
    replacements: &AtomicUsize,
) -> Result<(), NativeV2CliError> {
    if replacements
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(update_error(
            "could not replace this executable: No such file or directory (os error 2)",
        ));
    }
    fs::copy(staged, current)?;
    Ok(())
}

fn read_text(path: &Path) -> String {
    String::from_utf8_lossy(&fs::read(path).unwrap()).into_owned()
}

fn staged_update_files(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".zeroshot-update-") || name.starts_with(".restic-update-"))
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// A one-time event between the simulated update processes, awaited for at most `OVERLAP_WAIT`.
#[derive(Default)]
struct UpdateEvent {
    happened: Mutex<bool>,
    changed: Condvar,
}

impl UpdateEvent {
    fn signal(&self) {
        *self.happened.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let happened = self.happened.lock().unwrap();
        drop(
            self.changed
                .wait_timeout_while(happened, OVERLAP_WAIT, |happened| !*happened)
                .unwrap(),
        );
    }
}
