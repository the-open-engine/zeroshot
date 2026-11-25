use super::*;
use std::collections::BTreeMap;
use openengine_cluster_protocol::{FieldName, PublishedFile, PublishedFiles, UnixTimestampMillis};
use crate::execution::driver::DriverCancellation;
use crate::native_v2_files::publication::{
    LocalFilePublisher, RunFilePublisher, open_local_publication,
};

fn transfer(bytes: &[u8]) -> std::fs::File {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes).unwrap();
    std::io::Seek::rewind(&mut file).unwrap();
    file
}
fn now(value: u64) -> UnixTimestampMillis {
    UnixTimestampMillis::new(value).unwrap()
}
fn paths() -> BTreeMap<FieldName, String> {
    BTreeMap::from([(FieldName::new("report").unwrap(), "report".to_owned())])
}
fn address_of(files: &PublishedFiles) -> FileAddress {
    let PublishedFile::Managed { address, .. } = files.values().next().unwrap() else {
        panic!("managed receipt")
    };
    address.clone()
}
fn read_member(
    disk: &DiskFileStore,
    access: &FileAccess<'_>,
    address: FileAddress,
    member: &str,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    disk.download(
        access,
        &FileSelection {
            address,
            member: path(member),
        },
        &mut bytes,
    )
    .unwrap();
    let temp = tempdir();
    unpack(&mut transfer(&bytes), &temp.path().join("out")).unwrap();
    std::fs::read(temp.path().join("out")).unwrap()
}

#[test]
fn request_and_final_capture_distinct_complete_trees_and_replay_after_workspace_cleanup() {
    let temp = tempdir();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(workspace.join("report/empty")).unwrap();
    std::fs::write(workspace.join("report/data"), b"before").unwrap();
    let disk = store(temp.path());
    let identity = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &identity,
    };
    let (_sender, signal) = tokio::sync::watch::channel(false);
    let cancellation = DriverCancellation::new(signal);
    let request_owner = FilePublicationOwner::Request {
        run_id: pin().run_id,
        request: operation("question"),
    };
    let first = disk
        .capture_publication(
            &access,
            &workspace,
            &crate::native_v2_files::publication::FilePublication {
                owner: request_owner.clone(),
                paths: paths(),
                now: now(1),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap();
    std::fs::write(workspace.join("report/data"), b"after").unwrap();
    let final_owner = FilePublicationOwner::Final {
        run_id: pin().run_id,
    };
    let last = disk
        .capture_publication(
            &access,
            &workspace,
            &crate::native_v2_files::publication::FilePublication {
                owner: final_owner,
                paths: paths(),
                now: now(2),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap();
    assert_eq!(
        read_member(&disk, &access, address_of(&first), "data"),
        b"before"
    );
    assert_eq!(
        read_member(&disk, &access, address_of(&last), "data"),
        b"after"
    );
    let entries = disk
        .list_members(
            &access,
            &FileSelection {
                address: address_of(&first),
                member: path(""),
            },
        )
        .unwrap();
    assert!(entries.iter().any(|entry| entry.path == path("empty")));
    assert!(address_of(&first).path.as_str().ends_with("/report/report"));
    std::fs::remove_dir_all(&workspace).unwrap();
    drop(disk);
    let disk = store(temp.path());
    // Models a controller crash after storage commit and before its ledger append.
    assert_eq!(
        disk.capture_publication(
            &access,
            &workspace,
            &crate::native_v2_files::publication::FilePublication {
                owner: request_owner,
                paths: paths(),
                now: now(3),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            }
        )
        .unwrap(),
        first
    );
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
}

#[tokio::test]
async fn local_publication_refuses_redirection_of_the_recorded_working_root() {
    let temp = tempdir();
    let workspace = temp.path().join("working");
    let sibling = temp.path().join("sibling");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(workspace.join("report"), b"inside").unwrap();
    std::fs::write(sibling.join("report"), b"outside").unwrap();
    let publisher = LocalFilePublisher {
        workspace: workspace.clone(),
    };
    let (_sender, signal) = tokio::sync::watch::channel(false);
    let publication = || crate::native_v2_files::publication::FilePublication {
        owner: FilePublicationOwner::Final {
            run_id: pin().run_id,
        },
        paths: paths(),
        now: now(1),
        cancellation: DriverCancellation::new(signal.clone()),
        preflight: Arc::new(|_| Ok(())),
    };
    let files = publisher.publish(publication()).await.unwrap();
    std::fs::rename(&workspace, temp.path().join("original")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&sibling, &workspace).unwrap();
    #[cfg(windows)]
    assert!(
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&workspace)
            .arg(&sibling)
            .output()
            .unwrap()
            .status
            .success()
    );
    let error = publisher.publish(publication()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("artifacts.report path \"report\"")
    );
    let mut bytes = Vec::new();
    assert!(open_local_publication(files.values().next().unwrap(), &mut bytes).is_err());
    assert!(bytes.is_empty());
    #[cfg(unix)]
    std::fs::remove_file(&workspace).unwrap();
    #[cfg(windows)]
    std::fs::remove_dir(&workspace).unwrap();
    assert_eq!(std::fs::read(sibling.join("report")).unwrap(), b"outside");
}

#[cfg(target_os = "linux")]
#[test]
fn local_reference_open_never_rebases_confinement_during_root_exchange() {
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    let temp = tempdir();
    let workspace = temp.path().join("working");
    let sibling = temp.path().join("sibling");
    let redirect = temp.path().join("redirect");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(workspace.join("report"), b"inside").unwrap();
    let sentinel = b"OUTSIDE_RECORDED_WORKING_ROOT_SENTINEL";
    std::fs::write(sibling.join("report"), sentinel).unwrap();
    std::os::unix::fs::symlink(&sibling, &redirect).unwrap();
    let reference = PublishedFile::Local {
        root: workspace.to_str().unwrap().to_owned(),
        path: "report".to_owned(),
    };
    open_local_publication(&reference, &mut Vec::new()).unwrap();
    let first = std::ffi::CString::new(workspace.as_os_str().as_bytes()).unwrap();
    let second = std::ffi::CString::new(redirect.as_os_str().as_bytes()).unwrap();
    let exchange = move || {
        assert_eq!(
            unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    first.as_ptr(),
                    libc::AT_FDCWD,
                    second.as_ptr(),
                    libc::RENAME_EXCHANGE,
                )
            },
            0
        );
    };
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let swapping = std::thread::spawn(move || {
        while !stopping.load(Ordering::Relaxed) {
            exchange();
            std::thread::yield_now();
        }
    });
    let mut escaped = false;
    for _ in 0..2000 {
        let mut bytes = Vec::new();
        let _ = open_local_publication(&reference, &mut bytes);
        escaped |= bytes.windows(sentinel.len()).any(|bytes| bytes == sentinel);
        if escaped {
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    swapping.join().unwrap();
    assert!(!escaped, "a live reference read outside the recorded root");
}

#[test]
fn retention_is_explicit_and_pins_outlive_atomic_publication_cleanup() {
    const DAYS_60: u64 = 60 * 24 * 60 * 60 * 1000;
    let temp = tempdir();
    std::fs::create_dir(temp.path().join("report")).unwrap();
    std::fs::write(temp.path().join("report/data"), b"retained").unwrap();
    let disk = store(temp.path());
    let identity = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &identity,
    };
    let (_sender, signal) = tokio::sync::watch::channel(false);
    let cancellation = DriverCancellation::new(signal);
    let owner = FilePublicationOwner::Final {
        run_id: pin().run_id,
    };
    let files = disk
        .capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner: owner.clone(),
                paths: paths(),
                now: now(1),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap();
    assert_eq!(
        disk.cleanup_publications(
            &access,
            &crate::native_v2_files::FileRetention {
                candidates: &disk
                    .select_publications(&access, now(99999999999), true)
                    .unwrap(),
                now: now(DAYS_60),
                all: false,
                protected: &Default::default()
            }
        )
        .unwrap(),
        0
    );
    drop(disk);
    let disk = store(temp.path()); // Startup and reads do not expire the publication.
    let address = address_of(&files);
    assert_eq!(
        read_member(&disk, &access, address.clone(), "data"),
        b"retained"
    );
    let pin = disk
        .pin(
            &access,
            &FilePinRequest {
                pin: FilePin {
                    run_id: RunId::new("accepted-input"),
                    operation: operation("member"),
                },
                source: FileSelection {
                    address: address.clone(),
                    member: path("data"),
                },
            },
        )
        .unwrap();
    assert_eq!(
        disk.cleanup_publications(
            &access,
            &crate::native_v2_files::FileRetention {
                candidates: &disk
                    .select_publications(&access, now(99999999999), true)
                    .unwrap(),
                now: now(DAYS_60 + 1),
                all: false,
                protected: &Default::default()
            }
        )
        .unwrap(),
        1
    );
    assert!(
        disk.download(
            &access,
            &FileSelection {
                address,
                member: path("")
            },
            &mut Vec::new()
        )
        .is_err()
    );
    assert!(
        disk.capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner,
                paths: paths(),
                now: now(DAYS_60 + 2),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            }
        )
        .is_err()
    );
    disk.materialize(&access, &pin, &temp.path().join("accepted"))
        .unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("accepted")).unwrap(),
        b"retained"
    );
    assert_eq!(
        disk.cleanup_publications(
            &access,
            &crate::native_v2_files::FileRetention {
                candidates: &disk
                    .select_publications(&access, now(99999999999), true)
                    .unwrap(),
                now: now(DAYS_60 + 1),
                all: true,
                protected: &Default::default()
            }
        )
        .unwrap(),
        0
    );
}

#[test]
fn required_set_failure_and_cancellation_publish_no_partial_names() {
    let temp = tempdir();
    std::fs::write(temp.path().join("report"), b"one").unwrap();
    let disk = store(temp.path());
    let identity = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &identity,
    };
    let (sender, signal) = tokio::sync::watch::channel(false);
    let cancellation = DriverCancellation::new(signal);
    let owner = FilePublicationOwner::Final {
        run_id: pin().run_id,
    };
    let mut selected = paths();
    selected.insert(FieldName::new("missing").unwrap(), "gone".to_owned());
    let error = disk
        .capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner: owner.clone(),
                paths: selected.clone(),
                now: now(1),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("artifacts.missing path \"gone\"")
    );
    sender.send_replace(true);
    assert_eq!(
        disk.capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner: owner.clone(),
                paths: paths(),
                now: now(1),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            }
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::Interrupted
    );
    assert!(
        disk.list(
            &access,
            FileScope::Run {
                run_id: pin().run_id
            },
            &path("")
        )
        .unwrap()
        .is_empty()
    );
    sender.send_replace(false);
    assert!(
        disk.capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner,
                paths: paths(),
                now: now(1),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            }
        )
        .is_ok()
    );
}

#[tokio::test]
async fn local_references_remain_live_confined_and_outside_managed_storage() {
    let temp = tempdir();
    let workspace = temp.path().join("working");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("report"), b"old").unwrap();
    std::fs::write(temp.path().join("sibling"), b"private").unwrap();
    let publisher = LocalFilePublisher {
        workspace: workspace.clone(),
    };
    let (_sender, signal) = tokio::sync::watch::channel(false);
    let cancellation = DriverCancellation::new(signal);
    let request = publisher
        .publish(crate::native_v2_files::publication::FilePublication {
            owner: FilePublicationOwner::Request {
                run_id: pin().run_id,
                request: operation("question"),
            },
            paths: paths(),
            now: now(1),
            cancellation: cancellation.clone(),
            preflight: Arc::new(|_| Ok(())),
        })
        .await
        .unwrap();
    let final_files = publisher
        .publish(crate::native_v2_files::publication::FilePublication {
            owner: FilePublicationOwner::Final {
                run_id: pin().run_id,
            },
            paths: paths(),
            now: now(2),
            cancellation: cancellation.clone(),
            preflight: Arc::new(|_| Ok(())),
        })
        .await
        .unwrap();
    assert_eq!(request, final_files);
    let reference = request.values().next().unwrap();
    std::fs::write(workspace.join("report"), b"current").unwrap();
    let mut bytes = Vec::new();
    open_local_publication(reference, &mut bytes).unwrap();
    unpack(&mut transfer(&bytes), &temp.path().join("download")).unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("download")).unwrap(),
        b"current"
    );
    let root = publisher
        .publish(crate::native_v2_files::publication::FilePublication {
            owner: FilePublicationOwner::Final {
                run_id: pin().run_id,
            },
            paths: BTreeMap::from([(
                FieldName::new("root").unwrap(),
                workspace.to_str().unwrap().to_owned(),
            )]),
            now: now(2),
            cancellation: cancellation.clone(),
            preflight: Arc::new(|_| Ok(())),
        })
        .await
        .unwrap();
    let mut bytes = Vec::new();
    open_local_publication(root.values().next().unwrap(), &mut bytes).unwrap();
    unpack(&mut transfer(&bytes), &temp.path().join("whole-root")).unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("whole-root/report")).unwrap(),
        b"current"
    );
    std::fs::remove_file(workspace.join("report")).unwrap();
    assert!(open_local_publication(reference, &mut Vec::new()).is_err());
    let sibling = BTreeMap::from([(FieldName::new("bad").unwrap(), "../sibling".to_owned())]);
    assert!(
        publisher
            .publish(crate::native_v2_files::publication::FilePublication {
                owner: FilePublicationOwner::Final {
                    run_id: pin().run_id
                },
                paths: sibling,
                now: now(3),
                cancellation,
                preflight: Arc::new(|_| Ok(())),
            })
            .await
            .is_err()
    );
    assert!(!temp.path().join("store").exists());
}

struct PublicationAuthority;
impl FileAuthorizer for PublicationAuthority {
    fn authorize(&self, request: FileAuthorization<'_>) -> Result<String> {
        Ok(match &request.selection.address.scope {
            FileScope::Run { run_id } => run_id.as_str().to_owned(),
            _ => "library".to_owned(),
        })
    }
    fn authorize_publication(
        &self,
        request: FileAuthorization<'_>,
        owner: &FilePublicationOwner,
    ) -> Result<()> {
        let principal = request.access.identity.principal().as_str();
        if principal == "writer"
            || matches!(
                (owner, principal),
                (FilePublicationOwner::Request { .. }, "request-reader")
                    | (FilePublicationOwner::Final { .. }, "run-reader")
            )
        {
            Ok(())
        } else {
            Err(denied())
        }
    }
}
#[test]
fn request_and_final_access_are_independent_and_cleanup_preserves_pending_work() {
    let temp = tempdir();
    std::fs::write(temp.path().join("report"), "private").unwrap();
    let disk = DiskFileStore::open(
        temp.path().join("store"),
        "target-one".to_owned(),
        Arc::new(PublicationAuthority),
    )
    .unwrap();
    let writer = identity("writer");
    let access = FileAccess {
        target: "target-one",
        identity: &writer,
    };
    let (_sender, signal) = tokio::sync::watch::channel(false);
    let cancellation = DriverCancellation::new(signal);
    let request = disk
        .capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner: FilePublicationOwner::Request {
                    run_id: pin().run_id,
                    request: operation("request"),
                },
                paths: paths(),
                now: now(1),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap();
    let final_files = disk
        .capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner: FilePublicationOwner::Final {
                    run_id: RunId::new("final-run"),
                },
                paths: paths(),
                now: now(2),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap();
    for (principal, allowed, denied_files) in [
        ("request-reader", &request, &final_files),
        ("run-reader", &final_files, &request),
    ] {
        let reader = identity(principal);
        let reader = FileAccess {
            target: "target-one",
            identity: &reader,
        };
        assert_eq!(
            read_member(&disk, &reader, address_of(allowed), ""),
            b"private"
        );
        assert!(
            disk.download(
                &reader,
                &FileSelection {
                    address: address_of(denied_files),
                    member: path("")
                },
                &mut Vec::new()
            )
            .is_err()
        );
        assert!(
            disk.list(&reader, address_of(denied_files).scope, &path(""))
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        disk.cleanup_publications(
            &access,
            &crate::native_v2_files::FileRetention {
                candidates: &disk
                    .select_publications(&access, now(99999999999), true)
                    .unwrap(),
                now: now(3),
                all: true,
                protected: &[pin().run_id].into_iter().collect()
            }
        )
        .unwrap(),
        1
    );
    assert_eq!(
        read_member(&disk, &access, address_of(&request), ""),
        b"private"
    );
    assert_eq!(
        disk.cleanup_publications(
            &access,
            &crate::native_v2_files::FileRetention {
                candidates: &disk
                    .select_publications(&access, now(99999999999), true)
                    .unwrap(),
                now: now(4),
                all: true,
                protected: &Default::default()
            }
        )
        .unwrap(),
        1
    );
}

struct InterruptTransfer(std::sync::atomic::AtomicBool);
struct RemoveAfterFirstCapture(std::path::PathBuf);
impl FileAuthorizer for RemoveAfterFirstCapture {
    fn authorize(&self, request: FileAuthorization<'_>) -> Result<String> {
        if request.action == FileAction::Publish
            && request.selection.address.path.as_str().contains("/first/")
            && request.selection.member.as_str() == "file"
            && self.0.exists()
        {
            std::fs::remove_dir_all(&self.0)?;
        }
        Ok("partition".to_owned())
    }
}
impl FileAuthorizer for InterruptTransfer {
    fn authorize(&self, request: FileAuthorization<'_>) -> Result<String> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst)
            && request.action == FileAction::Publish
            && request.selection.address.path.as_str().contains("/second/")
            && request.selection.member.as_str() == "file"
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "transfer interrupted",
            ));
        }
        Ok("partition".to_owned())
    }
}
#[test]
fn interrupted_second_tree_never_exposes_the_first_and_the_same_identity_can_retry() {
    let temp = tempdir();
    std::fs::create_dir_all(temp.path().join("report")).unwrap();
    std::fs::write(temp.path().join("report/file"), "complete").unwrap();
    let authorization = Arc::new(InterruptTransfer(true.into()));
    let disk = DiskFileStore::open(
        temp.path().join("store"),
        "target-one".to_owned(),
        authorization.clone(),
    )
    .unwrap();
    let identity = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &identity,
    };
    let selected: BTreeMap<FieldName, String> = [
        (FieldName::new("first").unwrap(), "report".to_owned()),
        (FieldName::new("second").unwrap(), "report".to_owned()),
    ]
    .into_iter()
    .collect();
    let (_sender, signal) = tokio::sync::watch::channel(false);
    let cancellation = DriverCancellation::new(signal);
    let owner = FilePublicationOwner::Final {
        run_id: pin().run_id,
    };
    assert_eq!(
        disk.capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner: owner.clone(),
                paths: selected.clone(),
                now: now(1),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            }
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::ConnectionReset
    );
    assert!(
        disk.list(
            &access,
            FileScope::Run {
                run_id: pin().run_id
            },
            &path("")
        )
        .unwrap()
        .is_empty()
    );
    authorization
        .0
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        disk.capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner,
                paths: selected.clone(),
                now: now(2),
                cancellation: cancellation.clone(),
                preflight: Arc::new(|_| Ok(())),
            }
        )
        .unwrap()
        .len(),
        2
    );
    // All paths passed the initial set check. Removing the second source just before packing
    // still identifies its authored field/path and cannot expose the already transferred first.
    let disk = DiskFileStore::open(
        temp.path().join("race-store"),
        "target-one".to_owned(),
        Arc::new(RemoveAfterFirstCapture(temp.path().join("report"))),
    )
    .unwrap();
    let error = disk
        .capture_publication(
            &access,
            temp.path(),
            &crate::native_v2_files::publication::FilePublication {
                owner: FilePublicationOwner::Final {
                    run_id: pin().run_id,
                },
                paths: selected,
                now: now(3),
                cancellation,
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(
        error
            .to_string()
            .contains("artifacts.second path \"report\"")
    );
    assert!(
        disk.list(
            &access,
            FileScope::Run {
                run_id: pin().run_id
            },
            &path("")
        )
        .unwrap()
        .is_empty()
    );
}
