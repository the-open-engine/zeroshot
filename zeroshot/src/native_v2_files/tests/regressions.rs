use std::ffi::OsStr;
#[cfg(unix)]
use std::io::Seek;

use sha2::{Digest, Sha256};

use super::*;

#[test]
fn publication_keeps_parent_guards_and_cleans_stages_after_collisions() {
    let temp = tempdir();
    let directory = paths::Directory::open(temp.path()).unwrap();
    for value in [b"first".as_slice(), b"replacement".as_slice()] {
        let mut staged = paths::TemporaryFile::new(&directory).unwrap();
        staged.file.write_all(value).unwrap();
        staged.commit(OsStr::new("catalog")).unwrap();
        assert_eq!(std::fs::read(temp.path().join("catalog")).unwrap(), value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_WRITE_DATA,
        };
        let error = std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_DATA)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(temp.path())
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(32), "parent must remain guarded");
    }
    let source = temp.path().join("source");
    std::fs::create_dir_all(source.join("empty")).unwrap();
    std::fs::write(source.join("payload"), b"complete bytes").unwrap();
    let mut transfer = tempfile::tempfile().unwrap();
    pack(&source, &mut transfer).unwrap();
    transfer::unpack_at(&mut transfer, &directory, OsStr::new("output")).unwrap();
    assert!(transfer::unpack_at(&mut transfer, &directory, OsStr::new("output")).is_err());
    assert_eq!(
        std::fs::read(temp.path().join("output/payload")).unwrap(),
        b"complete bytes"
    );
    assert!(temp.path().join("output/empty").is_dir());
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&bytes(b"must not replace the tree"))
        .unwrap();
    assert!(transfer::unpack_at(&mut file, &directory, OsStr::new("output")).is_err());
    let mut entries: Vec<_> = directory.entries().unwrap().map(Result::unwrap).collect();
    entries.sort();
    assert_eq!(entries, ["catalog", "output", "source"]);
}

#[test]
fn noncanonical_headers_keep_original_offsets_for_extraction_and_projection() {
    let temp = tempdir();
    let mut transfer = tempfile::tempfile().unwrap();
    transfer.write_all(FILE_TREE_MAGIC).unwrap();
    for (header, body) in [
        ("{\n  \"kind\": \"directory\", \"path\": \"\"\n}", None),
        ("{ \"path\": \"empty\", \"kind\": \"directory\" }", None),
        (
            "{ \"path\": \"a\\u002ebin\", \"kind\": \"file\", \"byteLength\": 5 }",
            Some(&[0, 255, 1, 0, 254][..]),
        ),
        (
            "{\n \"path\": \"b.bin\", \"kind\": \"file\", \"byteLength\": 3\n}",
            Some(&[42, 0, 128][..]),
        ),
    ] {
        transfer
            .write_all(&(header.len() as u32).to_be_bytes())
            .unwrap();
        transfer.write_all(header.as_bytes()).unwrap();
        if let Some(body) = body {
            transfer.write_all(body).unwrap();
            transfer.write_all(&Sha256::digest(body)).unwrap();
        }
    }
    transfer.write_all(&0_u32.to_be_bytes()).unwrap();
    let destination = temp.path().join("tree");
    unpack(&mut transfer, &destination).unwrap();
    assert_eq!(
        std::fs::read(destination.join("a.bin")).unwrap(),
        [0, 255, 1, 0, 254]
    );
    assert_eq!(
        std::fs::read(destination.join("b.bin")).unwrap(),
        [42, 0, 128]
    );
    assert!(destination.join("empty").is_dir());
    let mut projected = tempfile::tempfile().unwrap();
    transfer::project(&mut transfer, &path("b.bin"), &mut projected, &mut |_| {
        Ok(())
    })
    .unwrap();
    unpack(&mut projected, &temp.path().join("member")).unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("member")).unwrap(),
        [42, 0, 128]
    );
}

#[cfg(unix)]
#[test]
fn extraction_and_cleanup_stay_in_the_opened_parent_after_path_redirection() {
    use std::os::unix::fs::symlink;
    let temp = tempdir();
    let parent_path = temp.path().join("parent");
    let original = temp.path().join("original");
    let outside = temp.path().join("outside");
    std::fs::create_dir(&parent_path).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let parent = paths::Directory::open(&parent_path).unwrap();
    std::fs::rename(&parent_path, &original).unwrap();
    symlink(&outside, &parent_path).unwrap();
    let mut input = tempfile::tempfile().unwrap();
    input.write_all(&bytes(b"private bytes\0\xff")).unwrap();
    transfer::unpack_at(&mut input, &parent, OsStr::new("output")).unwrap();
    assert_eq!(
        std::fs::read(original.join("output")).unwrap(),
        b"private bytes\0\xff"
    );
    // Existing destinations reject publication, then clean only the original stage directory.
    assert!(transfer::unpack_at(&mut input, &parent, OsStr::new("output")).is_err());
    assert_eq!(std::fs::read_dir(&original).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn extraction_directory_guard_prevents_parent_redirection() {
    let temp = tempdir();
    let parent_path = temp.path().join("parent");
    std::fs::create_dir(&parent_path).unwrap();
    let parent = paths::Directory::open(&parent_path).unwrap();
    assert!(std::fs::rename(&parent_path, temp.path().join("redirected")).is_err());
    let mut input = tempfile::tempfile().unwrap();
    input.write_all(&bytes(b"private bytes")).unwrap();
    transfer::unpack_at(&mut input, &parent, OsStr::new("output")).unwrap();
    assert_eq!(
        std::fs::read(parent_path.join("output")).unwrap(),
        b"private bytes"
    );
    assert!(transfer::unpack_at(&mut input, &parent, OsStr::new("output")).is_err());
    assert_eq!(std::fs::read_dir(&parent_path).unwrap().count(), 1);
}

#[test]
fn reclamation_refuses_a_redirected_objects_directory_and_preserves_external_sentinel() {
    let temp = tempdir();
    let disk = store(temp.path());
    let outside = temp.path().join("outside");
    paths::private_root(&outside).unwrap();
    let sentinel = outside.join("stage-external-sentinel");
    crate::execution::platform::private_file(
        &sentinel,
        crate::execution::platform::FileAccess::CreateNew,
    )
    .unwrap()
    .write_all(b"untouched")
    .unwrap();
    let objects = temp.path().join("store/objects");
    std::fs::rename(&objects, temp.path().join("original-objects")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &objects).unwrap();
    #[cfg(windows)]
    {
        let result = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&objects)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    assert!(disk.remove(&access, &address()).is_err());
    assert!(disk.reclaim().is_err());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"untouched");
    #[cfg(windows)]
    std::fs::remove_dir(&objects).unwrap();
}

#[cfg(unix)]
#[test]
fn transfer_drop_and_reclamation_keep_the_original_directory_capability() {
    use std::os::unix::fs::symlink;
    let temp = tempdir();
    let transfers = temp.path().join("transfers");
    let mut active = new_transfer(&transfers).unwrap();
    active.write_all(b"active").unwrap();
    active.rewind().unwrap();
    let name = std::fs::read_dir(&transfers)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .find(|name| name.to_string_lossy().starts_with("transfer-"))
        .unwrap();
    let original = temp.path().join("original-transfers");
    std::fs::rename(&transfers, &original).unwrap();
    let outside = temp.path().join("outside");
    paths::private_root(&outside).unwrap();
    std::fs::write(outside.join(&name), b"external sentinel").unwrap();
    symlink(&outside, &transfers).unwrap();
    assert!(reclaim_transfers(&transfers).is_err());
    drop(active);
    assert!(!original.join(&name).exists());
    assert_eq!(
        std::fs::read(outside.join(&name)).unwrap(),
        b"external sentinel"
    );
}

#[test]
fn incoming_and_normalized_transfer_limits_include_headers_digests_and_terminator() {
    use openengine_cluster_protocol::MAX_FILE_TRANSFER_BYTES;
    let body_length = (MAX_FILE_TRANSFER_BYTES / 1000) * 1000;
    let header = format!(
        "{{\"kind\":\"file\",\"path\":\"payload\",\"byteLength\":{}e3}}",
        body_length / 1000
    );
    let entry: FileEntry = serde_json::from_str(&header).unwrap();
    assert_eq!(serde_json::to_vec(&entry).unwrap().len(), header.len() + 1);
    for (extra, expected) in [
        (0, std::io::ErrorKind::UnexpectedEof),
        (1, std::io::ErrorKind::InvalidData),
        (2, std::io::ErrorKind::InvalidData),
    ] {
        let mut prefix = FILE_TREE_MAGIC.to_vec();
        for name in [
            String::new(),
            "a".repeat(128),
            "b".repeat(128),
            "c".repeat(128),
        ] {
            transfer::write_header(
                &mut prefix,
                &FileEntry {
                    path: path(&name),
                    kind: FileEntryKind::Directory,
                },
            )
            .unwrap();
        }
        // Pad with a real directory entry so incoming sizes are limit-1, limit and limit+1.
        // Canonicalizing the exponent adds one byte, independently of incoming framing.
        let prefix_size = MAX_FILE_TRANSFER_BYTES - body_length - 32 - 4 - 1 + extra;
        let empty = FileEntry {
            path: path(""),
            kind: FileEntryKind::Directory,
        };
        let overhead =
            prefix.len() + 4 + serde_json::to_vec(&empty).unwrap().len() + 4 + header.len();
        let name_length = usize::try_from(prefix_size).unwrap() - overhead;
        assert!((1..=128).contains(&name_length));
        transfer::write_header(
            &mut prefix,
            &FileEntry {
                path: path(&"d".repeat(name_length)),
                kind: FileEntryKind::Directory,
            },
        )
        .unwrap();
        prefix.extend((header.len() as u32).to_be_bytes());
        prefix.extend(header.as_bytes());
        assert_eq!(prefix.len() as u64, prefix_size);
        // No terabyte body is needed: an admissible declaration reaches EOF, while either
        // oversized representation must fail its budget before attempting to read that body.
        assert_eq!(
            validate_transfer(&mut Cursor::new(prefix))
                .unwrap_err()
                .kind(),
            expected
        );
    }
}
