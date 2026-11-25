use std::io::{Cursor, Read, Write};
use std::sync::{Arc, Barrier};

use openengine_cluster_protocol::{FileEntryKind, FILE_TREE_MAGIC};
use openengine_cluster_server::identity::{
    BindingAttributes, ConnectionIdentityConfig, PrincipalId, TenantId,
};
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().assert_value()).assert_value()
}

fn identity(principal: &str) -> ConnectionIdentity {
    ConnectionIdentity::new(ConnectionIdentityConfig {
        principal: PrincipalId::new(principal),
        tenant: TenantId::new("tenant"),
        issued_at_ms: None,
        expires_at_ms: u64::MAX,
        binding_attributes: BindingAttributes::default(),
    })
}
fn scope() -> FileScope {
    FileScope::User
}
fn path(value: &str) -> FilePath {
    FilePath::new(value).assert_value()
}
fn address() -> FileAddress {
    FileAddress {
        scope: scope(),
        path: path("library/data"),
    }
}
fn selection() -> FileSelection {
    FileSelection {
        address: address(),
        member: path(""),
    }
}
fn operation(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).assert_value()
}
fn pin() -> FilePin {
    FilePin {
        run_id: RunId::new("run-one"),
        operation: operation("input-one"),
    }
}
fn policy() -> Arc<dyn FileAuthorizer> {
    Arc::new(SingleAccountFiles {
        principal: "user".to_owned(),
        tenant: "tenant".to_owned(),
    })
}
fn store(root: &Path) -> DiskFileStore {
    DiskFileStore::open(root.join("store"), "target-one".to_owned(), policy())
        .unwrap_or_else(|error| panic!("open disk file store: {error:?}"))
}
fn bytes(value: &[u8]) -> Vec<u8> {
    let temp = tempdir();
    let source = temp.path().join("file");
    std::fs::write(&source, value).assert_value();
    let mut output = Vec::new();
    pack(&source, &mut output).unwrap();
    output
}
fn replace(store: &DiskFileStore, access: &FileAccess<'_>, value: &[u8], op: &str) {
    store
        .replace(
            access,
            &FileReplaceRequest {
                address: address(),
                operation: operation(op),
            },
            &mut Cursor::new(value),
        )
        .assert_value();
}
fn read(store: &DiskFileStore, access: &FileAccess<'_>) -> Vec<u8> {
    let mut result = Vec::new();
    store
        .download(access, &selection(), &mut result)
        .assert_value();
    result
}

#[test]
fn binary_nested_and_empty_trees_survive_reopen_and_member_selection() {
    let temp = tempdir();
    let source = temp.path().join("source");
    for directory in ["a", "b", "empty"] {
        std::fs::create_dir_all(source.join(directory)).assert_value();
    }
    std::fs::write(source.join("a/same.bin"), [0, 255, 1, 0, 254]).assert_value();
    std::fs::write(source.join("b/same.bin"), [42, 0, 128]).assert_value();
    let mut encoded = Vec::new();
    pack(&source, &mut encoded).unwrap();
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    let disk = store(temp.path());
    replace(&disk, &access, &encoded, "upload");
    assert_eq!(
        disk.list(&access, scope(), &path("")).assert_value(),
        vec![address()]
    );
    let entries = disk.list_members(&access, &selection()).assert_value();
    assert!(
        entries
            .iter()
            .any(|entry| entry.path == path("empty") && entry.kind == FileEntryKind::Directory)
    );
    assert_eq!(entries.len(), 6);
    drop(disk);
    let disk = store(temp.path());
    assert_eq!(read(&disk, &access), encoded);
    let destination = temp.path().join("download");
    let mut archive = tempfile::tempfile().assert_value();
    archive.write_all(&read(&disk, &access)).assert_value();
    unpack(&mut archive, &destination).assert_value();
    assert_eq!(
        std::fs::read(destination.join("a/same.bin")).assert_value(),
        [0, 255, 1, 0, 254]
    );
    assert_eq!(
        std::fs::read(destination.join("b/same.bin")).assert_value(),
        [42, 0, 128]
    );
    assert!(destination.join("empty").is_dir());
    assert!(unpack(&mut archive, &destination).is_err());
    let member = FileSelection {
        address: address(),
        member: path("b/same.bin"),
    };
    let mut downloaded = Vec::new();
    disk.download(&access, &member, &mut downloaded)
        .assert_value();
    assert_eq!(downloaded, bytes(&[42, 0, 128]));
    let empty = FileSelection {
        address: address(),
        member: path("empty"),
    };
    let members = disk.list_members(&access, &empty).assert_value();
    assert_eq!(
        members,
        vec![FileEntry {
            path: path(""),
            kind: FileEntryKind::Directory
        }]
    );
}

#[test]
fn replacement_racing_pin_selects_one_whole_tree_and_retries_keep_it_after_delete() {
    let temp = tempdir();
    let disk = store(temp.path());
    let first = bytes(&vec![1; 131_111]);
    let second = bytes(&vec![2; 131_111]);
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    replace(&disk, &access, &first, "first");
    let barrier = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let worker_barrier = barrier.clone();
        let disk = &disk;
        let second = &second;
        scope.spawn(move || {
            let who = identity("user");
            let access = FileAccess {
                target: "target-one",
                identity: &who,
            };
            worker_barrier.wait();
            replace(disk, &access, second, "second");
        });
        barrier.wait();
        disk.pin(
            &access,
            &FilePinRequest {
                pin: pin(),
                source: selection(),
            },
        )
        .assert_value();
    });
    let mut pinned = Vec::new();
    disk.read_pin(&access, &pin(), &mut pinned).assert_value();
    assert!(pinned == first || pinned == second);
    disk.remove(&access, &address()).assert_value();
    disk.pin(
        &access,
        &FilePinRequest {
            pin: pin(),
            source: selection(),
        },
    )
    .assert_value();
    let reopened = store(temp.path());
    let mut stable = Vec::new();
    reopened
        .read_pin(&access, &pin(), &mut stable)
        .assert_value();
    assert_eq!(stable, pinned);
    reopened
        .materialize(&access, &pin(), &temp.path().join("materialized"))
        .assert_value();
    assert_eq!(
        std::fs::read(temp.path().join("materialized"))
            .assert_value()
            .len(),
        131_111
    );
    reopened.release(&access, &pin()).assert_value();
    reopened.release(&access, &pin()).assert_value();
    assert!(reopened.read_pin(&access, &pin(), &mut Vec::new()).is_err());
    assert!(
        reopened
            .pin(
                &access,
                &FilePinRequest {
                    pin: pin(),
                    source: selection()
                }
            )
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("store/objects"))
            .assert_value()
            .count(),
        0
    );
}

struct Interrupted {
    input: Cursor<Vec<u8>>,
    left: usize,
}
impl Read for Interrupted {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "cancelled",
            ));
        }
        let len = buffer.len().min(self.left);
        let count = self.input.read(&mut buffer[..len])?;
        self.left -= count;
        Ok(count)
    }
}

#[test]
fn incomplete_cancelled_corrupt_and_conflicting_uploads_preserve_the_prior_selection() {
    let temp = tempdir();
    let disk = store(temp.path());
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    let first = bytes(b"retained");
    let complete = bytes(&vec![99; 100_000]);
    replace(&disk, &access, &first, "initial");
    for cutoff in [0, 8, 12, complete.len() / 2, complete.len() - 1] {
        assert!(
            disk.replace(
                &access,
                &FileReplaceRequest {
                    address: address(),
                    operation: operation("truncated")
                },
                &mut Cursor::new(&complete[..cutoff])
            )
            .is_err()
        );
        assert_eq!(read(&disk, &access), first);
    }
    assert!(
        disk.replace(
            &access,
            &FileReplaceRequest {
                address: address(),
                operation: operation("cancelled")
            },
            &mut Interrupted {
                input: Cursor::new(complete.clone()),
                left: 1000
            }
        )
        .is_err()
    );
    let mut corrupt = complete.clone();
    let index = corrupt.len() - 10;
    corrupt[index] ^= 1;
    assert!(
        disk.replace(
            &access,
            &FileReplaceRequest {
                address: address(),
                operation: operation("corrupt")
            },
            &mut Cursor::new(corrupt)
        )
        .is_err()
    );
    replace(&disk, &access, &first, "initial");
    assert!(
        disk.replace(
            &access,
            &FileReplaceRequest {
                address: address(),
                operation: operation("initial")
            },
            &mut Cursor::new(complete)
        )
        .is_err()
    );
    drop(disk);
    let disk = store(temp.path());
    assert_eq!(read(&disk, &access), first);
    assert_eq!(
        std::fs::read_dir(temp.path().join("store/objects"))
            .assert_value()
            .count(),
        1
    );
}

#[test]
fn staged_inputs_and_managed_publications_have_separate_idempotent_ownership() {
    let temp = tempdir();
    let disk = store(temp.path());
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    let content = bytes(b"run-private\0\xff");
    disk.stage(&access, &pin(), &mut Cursor::new(&content))
        .assert_value();
    disk.stage(&access, &pin(), &mut Cursor::new(&content))
        .assert_value();
    assert!(
        disk.stage(&access, &pin(), &mut Cursor::new(bytes(b"different")))
            .is_err()
    );
    let run_scope = FileScope::Run {
        run_id: pin().run_id,
    };
    assert!(
        disk.list(&access, run_scope.clone(), &path(""))
            .assert_value()
            .is_empty()
    );
    assert!(
        disk.list(&access, scope(), &path(""))
            .assert_value()
            .is_empty()
    );
    let request = FilePublishRequest {
        owner: FilePublicationOwner::Final {
            run_id: pin().run_id,
        },
        operation: operation("final-one"),
        name: path("result.bin"),
        pin: pin(),
    };
    let receipt = disk.publish(&access, &request).assert_value();
    assert_eq!(disk.publish(&access, &request).assert_value(), receipt);
    let HostFilePublication::Managed { address } = receipt else {
        panic!("managed receipt required")
    };
    disk.release(&access, &pin()).assert_value();
    assert_eq!(
        disk.publish(&access, &request).assert_value(),
        HostFilePublication::Managed {
            address: address.clone()
        }
    );
    assert_eq!(
        disk.list(&access, run_scope.clone(), &path(""))
            .assert_value(),
        vec![address.clone()]
    );
    assert!(disk.remove(&access, &address).is_err());
    assert!(
        disk.replace(
            &access,
            &FileReplaceRequest {
                address: address.clone(),
                operation: operation("forged")
            },
            &mut Cursor::new(&content)
        )
        .is_err()
    );
    let mut output = Vec::new();
    disk.download(
        &access,
        &FileSelection {
            address: address.clone(),
            member: path(""),
        },
        &mut output,
    )
    .assert_value();
    assert_eq!(output, content);
    assert_eq!(
        store(temp.path())
            .list(&access, run_scope, &path(""))
            .assert_value()
            .len(),
        1
    );
    // A live local reference is a host-only value; creating one never registers a run publication.
    let _live = HostFilePublication::LiveLocal {
        root: temp.path().to_owned(),
        member: path("source"),
    };
    assert_eq!(
        std::fs::read_dir(temp.path().join("store/objects"))
            .assert_value()
            .count(),
        1
    );
    assert!(disk.release_publication(&access, &address).assert_value());
    let another_pin = FilePin {
        run_id: pin().run_id,
        operation: operation("another-input"),
    };
    disk.stage(&access, &another_pin, &mut Cursor::new(bytes(b"new bytes")))
        .unwrap();
    let another_publication = FilePublishRequest {
        owner: request.owner.clone(),
        operation: operation("another-publication"),
        name: request.name.clone(),
        pin: another_pin,
    };
    assert!(disk.publish(&access, &another_publication).is_err());
    assert!(disk.publish(&access, &request).is_err());
    disk.release(&access, &another_publication.pin).unwrap();
    assert!(!disk.release_publication(&access, &address).assert_value());
    assert!(disk.publish(&access, &request).is_err());
    assert_eq!(
        std::fs::read_dir(temp.path().join("store/objects"))
            .assert_value()
            .count(),
        0
    );
}

struct DenyMember;
impl FileAuthorizer for DenyMember {
    fn authorize(&self, request: FileAuthorization<'_>) -> Result<String> {
        if request.selection.member.as_str().starts_with("secret")
            || request.selection.address.path.as_str() == "blocked"
            || matches!(request.selection.address.scope, FileScope::Org)
        {
            return Err(denied());
        }
        policy().authorize(request)
    }
}

#[test]
fn target_scope_identity_and_every_member_are_authorized_without_fallback() {
    let temp = tempdir();
    let disk = store(temp.path());
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    let source = temp.path().join("source");
    std::fs::create_dir(&source).assert_value();
    std::fs::write(source.join("secret"), b"private").assert_value();
    let mut content = Vec::new();
    pack(&source, &mut content).assert_value();
    replace(&disk, &access, &content, "seed");
    let guarded = DiskFileStore::open(
        temp.path().join("store"),
        "target-one".to_owned(),
        Arc::new(DenyMember),
    )
    .assert_value();
    assert!(
        guarded
            .download(&access, &selection(), &mut Vec::new())
            .is_err()
    );
    assert!(guarded.list_members(&access, &selection()).is_err());
    assert!(guarded.remove(&access, &address()).is_err());
    assert!(
        guarded
            .replace(
                &access,
                &FileReplaceRequest {
                    address: address(),
                    operation: operation("replace")
                },
                &mut Cursor::new(bytes(b"replacement"))
            )
            .is_err()
    );
    assert!(
        guarded
            .pin(
                &access,
                &FilePinRequest {
                    pin: pin(),
                    source: selection()
                }
            )
            .is_err()
    );
    assert!(guarded.list(&access, FileScope::Org, &path("")).is_err());
    assert!(
        guarded
            .list(&access, scope(), &path("blocked/allowed"))
            .is_err()
    );
    assert!(
        disk.list(
            &FileAccess {
                target: "another-target",
                identity: &who
            },
            scope(),
            &path("")
        )
        .is_err()
    );
    assert!(
        disk.list(
            &FileAccess {
                target: "target-one",
                identity: &identity("other-user")
            },
            scope(),
            &path("")
        )
        .is_err()
    );
    assert!(
        DiskFileStore::open(
            temp.path().join("store"),
            "another-target".to_owned(),
            policy()
        )
        .is_err()
    );
    assert_eq!(read(&disk, &access), content);
}

#[test]
fn malformed_members_and_windows_aliases_never_materialize() {
    for alias in [
        "../escape",
        "/absolute",
        "C:/drive",
        "a\\b",
        "a//b",
        "a/./b",
        "a/../b",
        "NUL",
        "CON .txt",
        "NUL .bin",
        "nul.txt",
        "COM1.txt",
        "LPT9",
        "a.",
        "a ",
        "a:b",
        "//server/share",
        "\\\\?\\C:\\test",
        "a\0b",
    ] {
        assert!(FilePath::new(alias).is_err(), "{alias}");
    }
    for member in ["../escape", "a\\b", "CON", "x."] {
        let mut archive = FILE_TREE_MAGIC.to_vec();
        let header = serde_json::json!({"path": member,"kind":"directory"}).to_string();
        archive.extend_from_slice(&(header.len() as u32).to_be_bytes());
        archive.extend(header.as_bytes());
        archive.extend_from_slice(&0_u32.to_be_bytes());
        assert!(validate_transfer(&mut Cursor::new(archive)).is_err());
    }
    let mut archive = FILE_TREE_MAGIC.to_vec();
    for entry in [
        FileEntry {
            path: path(""),
            kind: FileEntryKind::Directory,
        },
        FileEntry {
            path: path("A"),
            kind: FileEntryKind::Directory,
        },
        FileEntry {
            path: path("a"),
            kind: FileEntryKind::Directory,
        },
    ] {
        transfer::write_header(&mut archive, &entry).assert_value();
    }
    archive.extend_from_slice(&0_u32.to_be_bytes());
    assert!(validate_transfer(&mut Cursor::new(archive)).is_err());
    // Refuse the declared total before attempting to consume an enormous body.
    let mut oversized = FILE_TREE_MAGIC.to_vec();
    transfer::write_header(
        &mut oversized,
        &FileEntry {
            path: path(""),
            kind: FileEntryKind::File {
                byte_length: openengine_cluster_protocol::ByteLength::new(
                    openengine_cluster_protocol::MAX_FILE_TRANSFER_BYTES,
                )
                .unwrap(),
            },
        },
    )
    .unwrap();
    assert_eq!(
        validate_transfer(&mut Cursor::new(oversized))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[cfg(unix)]
#[test]
fn filesystem_links_and_hardlinks_are_refused_without_touching_their_targets() {
    use std::os::unix::fs::symlink;
    let temp = tempdir();
    let outside = temp.path().join("outside");
    std::fs::write(&outside, b"untouched").assert_value();
    let source = temp.path().join("source");
    std::fs::create_dir(&source).assert_value();
    symlink(&outside, source.join("escape")).assert_value();
    assert!(pack(&source, &mut Vec::new()).is_err());
    std::fs::remove_file(source.join("escape")).assert_value();
    std::fs::hard_link(&outside, source.join("hard")).assert_value();
    assert!(pack(&source, &mut Vec::new()).is_err());
    let linked = temp.path().join("linked");
    symlink(&source, &linked).assert_value();
    assert!(DiskFileStore::open(linked.join("store"), "target-one".to_owned(), policy()).is_err());
    let disk = store(temp.path());
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    replace(&disk, &access, &bytes(b"data"), "seed");
    let object = std::fs::read_dir(temp.path().join("store/objects"))
        .assert_value()
        .next()
        .assert_value()
        .assert_value()
        .path();
    std::fs::remove_file(&object).assert_value();
    symlink(&outside, &object).assert_value();
    assert!(
        disk.download(&access, &selection(), &mut Vec::new())
            .is_err()
    );
    assert_eq!(std::fs::read(&outside).assert_value(), b"untouched");
}

#[test]
fn restart_reclaims_abandoned_transfers_and_objects_but_preserves_active_leases() {
    let temp = tempdir();
    let directory = temp.path().join("transfers");
    let active = new_transfer(&directory).assert_value();
    assert_eq!(reclaim_transfers(&directory).assert_value(), 0);
    let abandoned = directory.join("transfer-abandoned");
    // A process exit leaves the private file behind without its descriptor/lease.
    crate::execution::platform::private_file(
        &abandoned,
        crate::execution::platform::FileAccess::CreateNew,
    )
    .assert_value();
    assert_eq!(reclaim_transfers(&directory).assert_value(), 1);
    assert!(!abandoned.exists());
    assert_eq!(std::fs::read_dir(&directory).assert_value().count(), 2);
    drop(active);
    assert_eq!(std::fs::read_dir(&directory).assert_value().count(), 1);
    let disk = store(temp.path());
    let orphan = temp
        .path()
        .join("store/objects")
        .join(uuid::Uuid::now_v7().to_string());
    crate::execution::platform::private_file(
        &orphan,
        crate::execution::platform::FileAccess::CreateNew,
    )
    .assert_value();
    assert_eq!(disk.reclaim().assert_value(), 1);
}

#[test]
fn request_and_final_publications_keep_independent_names_and_operation_domains() {
    let temp = tempdir();
    let disk = store(temp.path());
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    let content = bytes(b"one logical input");
    disk.stage(&access, &pin(), &mut Cursor::new(&content))
        .unwrap();
    let mut request = FilePublishRequest {
        owner: FilePublicationOwner::Request {
            run_id: pin().run_id,
            request: operation("request-one"),
        },
        operation: operation("publish-one"),
        name: path("result.bin"),
        pin: pin(),
    };
    let first = disk.publish(&access, &request).unwrap();
    request.owner = FilePublicationOwner::Final {
        run_id: pin().run_id,
    };
    let final_result = disk.publish(&access, &request).unwrap();
    assert_ne!(first, final_result);
    assert_eq!(
        disk.list(
            &access,
            FileScope::Run {
                run_id: pin().run_id
            },
            &path("")
        )
        .unwrap()
        .len(),
        2
    );
    let cancelled = FilePublishRequest {
        owner: request.owner.clone(),
        operation: operation("cancelled-publication"),
        name: path("cancelled.bin"),
        pin: pin(),
    };
    let cancelled_address = FileAddress {
        scope: FileScope::Run {
            run_id: pin().run_id,
        },
        path: path("final/cancelled.bin"),
    };
    assert!(
        !disk
            .release_publication(&access, &cancelled_address)
            .unwrap()
    );
    assert!(disk.publish(&access, &cancelled).is_err());
    let cancelled_pin = FilePin {
        run_id: pin().run_id,
        operation: operation("cancelled-input"),
    };
    disk.release(&access, &cancelled_pin).unwrap();
    assert!(
        disk.stage(&access, &cancelled_pin, &mut Cursor::new(&content))
            .is_err()
    );
    request.name = path("changed.bin");
    assert!(disk.publish(&access, &request).is_err());
    disk.release(&access, &pin()).unwrap();
    assert!(
        disk.stage(&access, &pin(), &mut Cursor::new(&content))
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn search_only_source_ancestors_are_traversed_without_reading_their_directory() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir();
    let parent = temp.path().join("search-only");
    std::fs::create_dir(&parent).unwrap();
    let source = parent.join("file.bin");
    std::fs::write(&source, b"accessible source").unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o111)).unwrap();
    let mut transfer = Vec::new();
    let result = pack(&source, &mut transfer);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    result.unwrap();
    assert_eq!(transfer, bytes(b"accessible source"));
}

#[cfg(windows)]
#[test]
fn windows_directory_junctions_are_refused() {
    let temp = tempdir();
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.bin"), b"retained").unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let junction = source.join("junction");
    let result = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result = pack(&source, &mut Vec::new());
    std::fs::remove_dir(&junction).unwrap();
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(outside.join("secret.bin")).unwrap(),
        b"retained"
    );
}

mod regressions;

#[path = "tests/workspaces.rs"]
mod workspaces;
