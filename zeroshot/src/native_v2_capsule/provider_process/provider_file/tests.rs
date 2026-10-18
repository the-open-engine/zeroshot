use super::*;

#[test]
fn replacement_is_private_and_replaces_the_entire_document() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let path = write_provider_file(&home, "models.json", b"a long document").unwrap();
    write_provider_file(&home, "models.json", b"short").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"short");
    assert_eq!(fs::read_dir(&home).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn filenames_cannot_escape_the_private_home() {
    let root = tempfile::tempdir().unwrap();
    for name in ["", ".", "..", "../victim", "/victim", "nested/file"] {
        assert!(write_provider_file(root.path(), name, b"document").is_err());
    }
}

#[cfg(unix)]
#[test]
fn destination_links_never_modify_their_target() {
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("victim");
    fs::write(&victim, b"untouched").unwrap();
    for hard_link in [false, true] {
        let home = root.path().join(format!("home-{hard_link}"));
        fs::create_dir(&home).unwrap();
        let destination = home.join("models.json");
        if hard_link {
            fs::hard_link(&victim, &destination).unwrap();
        } else {
            std::os::unix::fs::symlink(&victim, &destination).unwrap();
        }
        write_provider_file(&home, "models.json", b"new document").unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"untouched");
        assert_eq!(fs::read(destination).unwrap(), b"new document");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn root_provider_document_rejects_linked_homes_and_assigns_the_home_owner() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    fs::create_dir(&home).unwrap();
    let link = root.path().join("linked-home");
    std::os::unix::fs::symlink(&home, &link).unwrap();
    assert!(write_provider_file(&link, "models.json", b"document").is_err());
    // The same test runs as root in the hosted boundary lane.
    // SAFETY: geteuid only reads the calling process's effective identity.
    if unsafe { libc::geteuid() } == 0 {
        std::os::unix::fs::chown(&home, Some(65_534), Some(65_534)).unwrap();
        let path = write_provider_file(&home, "models.json", b"document").unwrap();
        let metadata = fs::metadata(path).unwrap();
        assert_eq!((metadata.uid(), metadata.gid()), (65_534, 65_534));
    }
}
