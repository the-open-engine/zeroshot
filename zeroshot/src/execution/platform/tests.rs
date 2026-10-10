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

#[cfg(unix)]
#[test]
fn find_executable_takes_the_first_runnable_regular_file_on_path() {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    let root = TemporaryDirectory::for_test("find-executable");
    let [plain, directory, runnable, later] =
        ["plain", "directory", "runnable", "later"].map(|name| root.path(name));
    for path in [&plain, &directory, &runnable, &later] {
        fs::create_dir(path).unwrap();
    }
    fs::write(plain.join("harness"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(plain.join("harness"), fs::Permissions::from_mode(0o644)).unwrap();
    fs::create_dir(directory.join("harness")).unwrap();
    for bin in [&runnable, &later] {
        openengine_cluster_testkit::fixture::write_executable(
            &bin.join("harness"),
            "#!/bin/sh\n",
            0o755,
        )
        .unwrap();
    }
    let search_path = std::env::join_paths([&plain, &directory, &runnable, &later]).unwrap();
    let environment = BTreeMap::from([("PATH".to_owned(), search_path.into_string().unwrap())]);

    assert_eq!(
        find_executable("harness", &environment, root.as_path()),
        Some(runnable.join("harness"))
    );
    let direct = later.join("harness");
    assert_eq!(
        find_executable(direct.to_str().unwrap(), &BTreeMap::new(), root.as_path()),
        Some(direct)
    );
    assert_eq!(
        find_executable(
            plain.join("harness").to_str().unwrap(),
            &environment,
            root.as_path()
        ),
        None
    );
}

#[cfg(unix)]
#[test]
fn find_executable_is_none_without_a_match_or_a_search_path() {
    use std::collections::BTreeMap;

    let root = TemporaryDirectory::for_test("find-executable-none");
    let bin = root.path("bin");
    fs::create_dir(&bin).unwrap();
    openengine_cluster_testkit::fixture::write_executable(
        &bin.join("harness"),
        "#!/bin/sh\n",
        0o755,
    )
    .unwrap();
    let path = |value: &str| BTreeMap::from([("PATH".to_owned(), value.to_owned())]);
    let working_directory = root.as_path();

    assert_eq!(
        find_executable("harness", &path(bin.to_str().unwrap()), working_directory),
        Some(bin.join("harness"))
    );
    assert_eq!(
        find_executable("absent", &path(bin.to_str().unwrap()), working_directory),
        None
    );
    // An empty PATH is a single empty entry, the working directory, and it has no harness.
    assert_eq!(
        find_executable("harness", &path(""), working_directory),
        None
    );
    assert_eq!(
        find_executable("harness", &BTreeMap::new(), working_directory),
        None
    );
}

#[cfg(unix)]
#[test]
fn find_executable_follows_a_symlink_and_skips_a_dangling_one() {
    use std::collections::BTreeMap;

    let root = TemporaryDirectory::for_test("find-executable-symlink");
    let [store, dangling, linked] = ["store", "dangling", "linked"].map(|name| root.path(name));
    for path in [&store, &dangling, &linked] {
        fs::create_dir(path).unwrap();
    }
    let target = store.join("cli.js");
    openengine_cluster_testkit::fixture::write_executable(&target, "#!/bin/sh\n", 0o755).unwrap();
    std::os::unix::fs::symlink(store.join("removed"), dangling.join("harness")).unwrap();
    std::os::unix::fs::symlink(&target, linked.join("harness")).unwrap();
    let search_path = std::env::join_paths([&dangling, &linked]).unwrap();
    let environment = BTreeMap::from([("PATH".to_owned(), search_path.into_string().unwrap())]);

    assert_eq!(
        find_executable("harness", &environment, root.as_path()),
        Some(linked.join("harness"))
    );
}

#[cfg(unix)]
#[test]
fn search_directories_resolve_each_entry_against_the_working_directory() {
    use std::path::PathBuf;

    let working_directory = std::path::Path::new("/workspace");
    let directories = |path: &str| search_directories(path, working_directory).collect::<Vec<_>>();

    // Not normalized: `Path` equality compares components, so `/workspace/./tools` matches.
    assert_eq!(directories(""), [PathBuf::from("/workspace")]);
    assert_eq!(
        directories(":"),
        [PathBuf::from("/workspace"), PathBuf::from("/workspace")]
    );
    assert_eq!(
        directories("/first::bin:./tools:"),
        [
            PathBuf::from("/first"),
            PathBuf::from("/workspace"),
            PathBuf::from("/workspace/bin"),
            PathBuf::from("/workspace/tools"),
            PathBuf::from("/workspace"),
        ]
    );
}

#[cfg(unix)]
#[test]
fn find_executable_resolves_an_empty_path_entry_to_the_working_directory() {
    use std::collections::BTreeMap;

    let root = TemporaryDirectory::for_test("find-executable-empty-entry");
    let [workspace, elsewhere] = ["workspace", "elsewhere"].map(|name| root.path(name));
    for path in [&workspace, &elsewhere] {
        fs::create_dir(path).unwrap();
    }
    let environment = BTreeMap::from([("PATH".to_owned(), format!(":{}", elsewhere.display()))]);

    assert_eq!(find_executable("codex", &environment, &workspace), None);

    openengine_cluster_testkit::fixture::write_executable(
        &workspace.join("codex"),
        "#!/bin/sh\n",
        0o755,
    )
    .unwrap();
    assert_eq!(
        find_executable("codex", &environment, &workspace),
        Some(workspace.join("codex"))
    );
}

#[cfg(unix)]
#[test]
fn find_executable_joins_a_relative_path_entry_to_the_working_directory() {
    use std::collections::BTreeMap;

    let root = TemporaryDirectory::for_test("find-executable-relative-entry");
    let workspace = root.path("workspace");
    fs::create_dir_all(workspace.join("bin")).unwrap();
    let executable = workspace.join("bin").join("codex");
    openengine_cluster_testkit::fixture::write_executable(&executable, "#!/bin/sh\n", 0o755)
        .unwrap();
    let environment = BTreeMap::from([("PATH".to_owned(), "bin".to_owned())]);

    assert_eq!(
        find_executable("codex", &environment, &workspace),
        Some(executable.clone())
    );
    assert_eq!(find_executable("codex", &environment, root.as_path()), None);
    assert_eq!(
        find_executable("bin/codex", &BTreeMap::new(), &workspace),
        Some(executable)
    );
}

#[cfg(windows)]
#[test]
fn windows_find_executable_tries_only_the_fixed_suffixes() {
    use std::collections::BTreeMap;

    let root = TemporaryDirectory::for_test("find-executable-windows");
    let shim = root.path("shim");
    let bare = root.path("bare");
    fs::create_dir(&shim).unwrap();
    fs::create_dir(&bare).unwrap();
    fs::write(shim.join("harness"), "extensionless placeholder").unwrap();
    fs::write(shim.join("harness.cmd"), "@exit /b 0\r\n").unwrap();
    fs::write(bare.join("harness"), "extensionless placeholder").unwrap();
    let path = |directory: &std::path::Path| {
        BTreeMap::from([("Path".to_owned(), directory.to_str().unwrap().to_owned())])
    };

    assert_eq!(
        find_executable("harness", &path(&shim), root.as_path()),
        Some(shim.join("harness.cmd"))
    );
    assert_eq!(
        find_executable("harness", &path(&bare), root.as_path()),
        None
    );
    assert_eq!(
        executable("harness", &path(&shim)),
        shim.join("harness.cmd")
    );
    assert_eq!(
        executable("harness", &path(&bare)),
        std::path::PathBuf::from("harness")
    );
}
