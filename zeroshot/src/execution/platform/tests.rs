use std::fs;

use openengine_cluster_testkit::TemporaryDirectory;

use super::*;

#[test]
fn private_state_round_trips_and_replaces_an_existing_file() {
    let root = TemporaryDirectory::for_test("private-state");
    let directory = root.path("private");
    private_directory(&directory).unwrap();
    let destination = directory.join("state.json");
    for contents in [b"first".as_slice(), b"second"] {
        let temporary = directory.join("temporary");
        let mut file = private_file(&temporary, FileAccess::CreateNew).unwrap();
        std::io::Write::write_all(&mut file, contents).unwrap();
        file.sync_all().unwrap();
        drop(file);
        commit_file(&temporary, &destination, &directory).unwrap();
        let mut file = private_file(&destination, FileAccess::Read).unwrap();
        let mut actual = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut actual).unwrap();
        assert_eq!(actual, contents);
    }
}

#[test]
fn identity_distinguishes_a_replacement_at_the_same_path() {
    let root = TemporaryDirectory::for_test("identity");
    let directory = root.path("workspace");
    fs::create_dir(&directory).unwrap();
    let pinned = open_identity(&directory).unwrap();
    let original = file_identity(&pinned).unwrap();
    fs::rename(&directory, root.path("retired")).unwrap();
    fs::create_dir(&directory).unwrap();
    let replacement = open_identity(&directory).unwrap();
    assert_ne!(original, file_identity(&replacement).unwrap());
    assert_eq!(original, file_identity(&pinned).unwrap());
}

#[cfg(windows)]
#[test]
fn windows_rejects_junctions_and_nonprivate_files() {
    let root = TemporaryDirectory::for_test("acl");
    let outside = root.path("outside");
    fs::create_dir(&outside).unwrap();
    let junction = root.path("junction");
    assert!(
        std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(private_directory(&junction).is_err());
    assert!(open_identity(&junction).is_err());
    fs::remove_dir(&junction).unwrap();
    let file = root.path("secret");
    drop(private_file(&file, FileAccess::CreateNew).unwrap());
    assert!(
        std::process::Command::new("icacls.exe")
            .arg(&file)
            .args(["/grant", "*S-1-1-0:R"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(private_file(&file, FileAccess::Read).is_err());
}

#[cfg(unix)]
#[test]
fn private_state_and_workspace_reject_fifos_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let root = TemporaryDirectory::for_test("private-fifo");
    let path = root.path("fifo");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(open_directory(&path).is_err());
    assert!(private_file(&path, FileAccess::Read).is_err());
}

#[cfg(all(unix, feature = "ui"))]
#[test]
fn existing_read_write_probe_neither_repairs_nor_creates_files() {
    use std::os::unix::fs::PermissionsExt;

    let root = TemporaryDirectory::for_test("private-probe");
    let insecure = root.path("insecure");
    fs::write(&insecure, []).unwrap();
    fs::set_permissions(&insecure, fs::Permissions::from_mode(0o644)).unwrap();

    let error = private_file(&insecure, FileAccess::ReadWriteExisting).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(
        fs::metadata(&insecure).unwrap().permissions().mode() & 0o777,
        0o644
    );

    let missing = root.path("missing");
    assert_eq!(
        private_file(&missing, FileAccess::ReadWriteExisting)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(!missing.exists());
}

#[cfg(windows)]
#[test]
fn windows_preserves_local_profile_folders_and_isolates_private_homes() {
    use std::collections::BTreeMap;
    let home = std::env::var("USERPROFILE").unwrap();
    let mut local = BTreeMap::from([("HOME".to_owned(), home)]);
    process_environment(&mut local);
    for name in ["APPDATA", "LOCALAPPDATA"] {
        if let Ok(expected) = std::env::var(name) {
            assert_eq!(local[name], expected);
        }
    }
    let mut private = BTreeMap::from([("HOME".to_owned(), r"C:\private-home".to_owned())]);
    process_environment(&mut private);
    assert_eq!(private["USERPROFILE"], r"C:\private-home");
    assert_eq!(private["APPDATA"], r"C:\private-home\AppData\Roaming");
    assert_eq!(private["LOCALAPPDATA"], r"C:\private-home\AppData\Local");
}
