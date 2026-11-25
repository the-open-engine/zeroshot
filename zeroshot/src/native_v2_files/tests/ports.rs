use std::io::{Seek, SeekFrom};

use super::*;

#[test]
fn transfer_reclamation_refuses_unexpected_directories_without_losing_live_bytes() {
    let temp = tempdir();
    let transfers = temp.path().join("transfers");
    let mut transfer = new_transfer(&transfers).unwrap();
    transfer.write_all(b"live transfer").unwrap();
    let unexpected = transfers.join("transfer-directory");
    std::fs::create_dir(&unexpected).unwrap();
    std::fs::write(unexpected.join("retained"), b"unrelated bytes").unwrap();
    assert!(reclaim_transfers(&transfers).is_err());
    assert_eq!(
        std::fs::read(unexpected.join("retained")).unwrap(),
        b"unrelated bytes"
    );
    transfer.rewind().unwrap();
    let mut live = Vec::new();
    transfer.read_to_end(&mut live).unwrap();
    assert_eq!(live, b"live transfer");
    std::fs::remove_dir_all(unexpected).unwrap();
    assert_eq!(reclaim_transfers(&transfers).unwrap(), 0);
}

#[test]
fn leased_transfer_streams_stage_complete_bytes_and_remains_live_until_drop() {
    let temp = tempdir();
    let transfers = temp.path().join("transfers");
    let encoded = bytes(b"streamed binary\0\xff");
    let mut transfer = new_transfer(&transfers).unwrap();
    transfer.write_all(&encoded).unwrap();
    transfer.flush().unwrap();
    transfer.as_file_mut().sync_all().unwrap();
    assert_eq!(
        transfer.as_file().metadata().unwrap().len(),
        encoded.len() as u64
    );
    transfer.seek(SeekFrom::Start(0)).unwrap();
    let mut magic = [0; 8];
    transfer.read_exact(&mut magic).unwrap();
    assert_eq!(&magic, FILE_TREE_MAGIC);
    transfer.rewind().unwrap();

    let disk = store(temp.path());
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    disk.stage(&access, &pin(), &mut transfer).unwrap();
    assert_eq!(reclaim_transfers(&transfers).unwrap(), 0);
    let mut pinned = Vec::new();
    disk.read_pin(&access, &pin(), &mut pinned).unwrap();
    assert_eq!(pinned, encoded);
    drop(transfer);
    assert_eq!(reclaim_transfers(&transfers).unwrap(), 0);
    assert_eq!(std::fs::read_dir(&transfers).unwrap().count(), 1);
    disk.materialize(&access, &pin(), &temp.path().join("output"))
        .unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("output")).unwrap(),
        b"streamed binary\0\xff"
    );
}

#[test]
fn single_account_disk_scopes_are_distinct_and_both_identity_fields_are_required() {
    let temp = tempdir();
    let disk = store(temp.path());
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    for (scope, op, contents) in [
        (FileScope::User, "user-upload", b"user"),
        (FileScope::Org, "org-upload", b"team"),
    ] {
        let address = FileAddress {
            scope: scope.clone(),
            path: path("report"),
        };
        disk.replace(
            &access,
            &FileReplaceRequest {
                address: address.clone(),
                operation: operation(op),
            },
            &mut Cursor::new(bytes(contents)),
        )
        .unwrap();
        assert_eq!(disk.list(&access, scope, &path("")).unwrap(), vec![address]);
    }
    let wrong_tenant = ConnectionIdentity::new(ConnectionIdentityConfig {
        principal: PrincipalId::new("user"),
        tenant: TenantId::new("other-tenant"),
        issued_at_ms: None,
        expires_at_ms: u64::MAX,
        binding_attributes: BindingAttributes::default(),
    });
    let wrong_principal = identity("other-user");
    for (scope, contents) in [(FileScope::User, b"user"), (FileScope::Org, b"team")] {
        let selected = FileSelection {
            address: FileAddress {
                scope,
                path: path("report"),
            },
            member: path(""),
        };
        for who in [&wrong_tenant, &wrong_principal] {
            let refused = FileAccess {
                target: "target-one",
                identity: who,
            };
            let mut output = Vec::new();
            assert_eq!(
                disk.download(&refused, &selected, &mut output)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
            assert!(output.is_empty());
            assert_eq!(
                disk.remove(&refused, &selected.address).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        let mut output = Vec::new();
        disk.download(&access, &selected, &mut output).unwrap();
        assert_eq!(output, bytes(contents));
    }
}

/// Models an existing host that delegates file operations but has not opted into cleanup.
struct FilesWithoutCleanup<'a>(&'a DiskFileStore);

impl TargetFileHost for FilesWithoutCleanup<'_> {
    fn list(
        &self,
        access: &FileAccess<'_>,
        scope: FileScope,
        prefix: &FilePath,
    ) -> Result<Vec<FileAddress>> {
        self.0.list(access, scope, prefix)
    }
    fn list_members(
        &self,
        access: &FileAccess<'_>,
        selection: &FileSelection,
    ) -> Result<Vec<FileEntry>> {
        self.0.list_members(access, selection)
    }
    fn replace(
        &self,
        access: &FileAccess<'_>,
        request: &FileReplaceRequest,
        input: &mut dyn Read,
    ) -> Result<()> {
        self.0.replace(access, request, input)
    }
    fn download(
        &self,
        access: &FileAccess<'_>,
        selection: &FileSelection,
        output: &mut dyn Write,
    ) -> Result<()> {
        self.0.download(access, selection, output)
    }
    fn remove(&self, access: &FileAccess<'_>, address: &FileAddress) -> Result<bool> {
        self.0.remove(access, address)
    }
    fn pin(&self, access: &FileAccess<'_>, request: &FilePinRequest) -> Result<FilePin> {
        self.0.pin(access, request)
    }
    fn pin_exists(&self, access: &FileAccess<'_>, pin: &FilePin) -> Result<bool> {
        self.0.pin_exists(access, pin)
    }
    fn stage(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        input: &mut dyn Read,
    ) -> Result<FilePin> {
        self.0.stage(access, pin, input)
    }
    fn read_pin(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        output: &mut dyn Write,
    ) -> Result<()> {
        self.0.read_pin(access, pin, output)
    }
    fn materialize(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        destination: &Path,
    ) -> Result<()> {
        self.0.materialize(access, pin, destination)
    }
    fn publish(
        &self,
        access: &FileAccess<'_>,
        request: &FilePublishRequest,
    ) -> Result<HostFilePublication> {
        self.0.publish(access, request)
    }
    fn release_publication(&self, access: &FileAccess<'_>, address: &FileAddress) -> Result<bool> {
        self.0.release_publication(access, address)
    }
    fn release(&self, access: &FileAccess<'_>, pin: &FilePin) -> Result<()> {
        self.0.release(access, pin)
    }
    fn reclaim(&self) -> Result<usize> {
        self.0.reclaim()
    }
}

#[test]
fn hosts_without_cleanup_refuse_it_and_preserve_real_publications_and_pins() {
    use crate::execution::driver::DriverCancellation;
    use crate::native_v2_files::publication::FilePublication;
    use openengine_cluster_protocol::{FieldName, PublishedFile, UnixTimestampMillis};

    let temp = tempdir();
    std::fs::write(temp.path().join("report"), b"retained").unwrap();
    let disk = store(temp.path());
    let who = identity("user");
    let access = FileAccess {
        target: "target-one",
        identity: &who,
    };
    let field = FieldName::new("report").unwrap();
    let (_sender, signal) = tokio::sync::watch::channel(false);
    let files = disk
        .capture_publication(
            &access,
            temp.path(),
            &FilePublication {
                owner: FilePublicationOwner::Final {
                    run_id: pin().run_id,
                },
                paths: std::collections::BTreeMap::from([(field.clone(), "report".to_owned())]),
                now: UnixTimestampMillis::new(1).unwrap(),
                cancellation: DriverCancellation::new(signal),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap();
    let PublishedFile::Managed { address, .. } = &files[&field] else {
        panic!("managed publication")
    };
    let legacy = FilesWithoutCleanup(&disk);
    let accepted = legacy
        .pin(
            &access,
            &FilePinRequest {
                pin: pin(),
                source: FileSelection {
                    address: address.clone(),
                    member: path(""),
                },
            },
        )
        .unwrap();
    std::fs::remove_file(temp.path().join("report")).unwrap();
    let now = UnixTimestampMillis::new(61 * 24 * 60 * 60 * 1000).unwrap();
    let candidates = disk.select_publications(&access, now, true).unwrap();
    assert_eq!(candidates.len(), 1);
    for all in [false, true] {
        assert_eq!(
            legacy
                .select_publications(&access, now, all)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            legacy
                .cleanup_publications(
                    &access,
                    &FileRetention {
                        candidates: &candidates,
                        now,
                        all,
                        protected: &Default::default(),
                    }
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
    }
    let mut retained = Vec::new();
    legacy
        .download(
            &access,
            &FileSelection {
                address: address.clone(),
                member: path(""),
            },
            &mut retained,
        )
        .unwrap();
    assert_eq!(retained, bytes(b"retained"));
    drop(disk);
    let disk = store(temp.path());
    assert_eq!(
        disk.list(&access, address.scope.clone(), &path(""))
            .unwrap(),
        vec![address.clone()]
    );
    assert_eq!(
        disk.cleanup_publications(
            &access,
            &FileRetention {
                candidates: &candidates,
                now,
                all: true,
                protected: &Default::default(),
            }
        )
        .unwrap(),
        1
    );
    disk.materialize(&access, &accepted, &temp.path().join("accepted"))
        .unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("accepted")).unwrap(),
        b"retained"
    );
}
