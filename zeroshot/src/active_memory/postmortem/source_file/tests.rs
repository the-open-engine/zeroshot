use super::*;

#[test]
fn regular_source_is_pinned_and_identifier_matching_remains_exact() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    let source = root.path().join("src/provider.rs");
    std::fs::write(&source, "fn reject_provider_controls() {}\n").unwrap();
    let file = open(root.path(), "src/provider.rs").unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::write(&source, "replacement").unwrap();
    assert!(has_identifier(file, &["reject_provider_controls".into()]));
    assert!(!has_identifier(
        open(root.path(), "src/provider.rs").unwrap(),
        &["reject_provider_controls".into()]
    ));
    assert!(open(root.path(), "src").is_err());
    assert!(open(root.path(), "../outside").is_err());
}

#[cfg(unix)]
#[test]
fn source_fifos_and_escaping_symlinks_are_rejected_without_blocking() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "outside").unwrap();
    symlink(outside.path().join("secret"), root.path().join("link")).unwrap();
    symlink(outside.path(), root.path().join("parent")).unwrap();
    assert!(open(root.path(), "link").is_err());
    assert!(open(root.path(), "parent/secret").is_err());
    let fifo = root.path().join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a valid NUL-terminated path owned by this fixture.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(open(root.path(), "fifo").is_err());
    let directory = crate::execution::platform::open_directory(root.path()).unwrap();
    // Exercise the post-canonicalization walker directly against replacement entries.
    assert!(open_beneath(&directory, root.path(), Path::new("link")).is_err());
    assert!(open_beneath(&directory, root.path(), Path::new("parent/secret")).is_err());
    let replacement = open_beneath(&directory, root.path(), Path::new("fifo")).unwrap();
    assert!(!replacement.metadata().unwrap().is_file());
}
