use super::*;
use openengine_cluster_protocol::{PublishedFile, PublishedFiles, UnixTimestampMillis};

/// A receipt survives cleanup so replay cannot give a released identity different bytes.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Publication {
    pub(super) partition: String,
    pub(super) owner: FilePublicationOwner,
    pub(super) fingerprint: String,
    pub(super) published_at: UnixTimestampMillis,
    pub(super) files: PublishedFiles,
    pub(super) released: bool,
}

pub const PUBLICATION_RETENTION_MILLIS: u64 = 60 * 24 * 60 * 60 * 1000;

impl DiskFileStore {
    /// Capture the complete selected set and commit all names and its durable receipt together.
    /// Callers run this on the blocking pool and drain it even when their request is cancelled.
    pub fn capture_publication(
        &self,
        access: &FileAccess<'_>,
        workspace: &Path,
        request: &super::super::publication::FilePublication,
    ) -> Result<PublishedFiles> {
        let owner = request.owner.clone();
        let selected = &request.paths;
        let now = request.now;
        let cancellation = &request.cancellation;
        let operation = self.operation_key(access, "capture-publication", &owner)?;
        let fingerprint = key(selected)?;
        let (_lock, mut catalog) = self.load()?;
        if let Some(files) = self.publication_receipt_in(access, request, &catalog)? {
            (request.preflight)(&files)?;
            return Ok(files);
        }
        check_cancelled(cancellation)?;
        // Check every required path before importing any bytes. Packing rechecks each actual tree.
        let paths = selected
            .iter()
            .map(|(field, path)| {
                let resolved = super::super::workspace_path(workspace, path).map_err(|error| {
                    std::io::Error::new(
                        error.kind(),
                        format!("artifacts.{} path {path:?}: {error}", field.as_str()),
                    )
                })?;
                Ok((field, resolved))
            })
            .collect::<Result<Vec<_>>>()?;
        let prefix = match &owner {
            FilePublicationOwner::Request { request, .. } => format!("requests/{}", key(request)?),
            FilePublicationOwner::Final { .. } => "final".to_owned(),
        };
        let mut files = PublishedFiles::new();
        let mut publication_partition = None;
        let mut imports = Vec::with_capacity(paths.len());
        for (field, path) in paths {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("artifact must have a portable file or folder name"))?;
            let address = FileAddress {
                scope: FileScope::Run {
                    run_id: owner.run_id().clone(),
                },
                path: FilePath::new(format!("{prefix}/{}/{name}", field.as_str()))
                    .map_err(invalid)?,
            };
            let partition = self.selection(access, &address, FileAction::Publish)?;
            self.authorize_owner(
                FileAuthorization {
                    access,
                    selection: &FileSelection {
                        address: address.clone(),
                        member: root_path(),
                    },
                    action: FileAction::Publish,
                },
                Some(&owner),
            )?;
            if publication_partition
                .as_ref()
                .is_some_and(|previous| previous != &partition)
            {
                return Err(denied());
            }
            publication_partition = Some(partition.clone());
            let identity = key(&(&partition, &address))?;
            let name_key = key(&("publication-name", &partition, &address))?;
            if catalog.trees.contains_key(&identity) || catalog.operations.contains_key(&name_key) {
                return Err(invalid("managed publication name already used"));
            }
            files.insert(
                field.clone(),
                PublishedFile::Managed {
                    target: access.target.to_owned(),
                    address: address.clone(),
                },
            );
            imports.push((field, path, address, partition, identity, name_key));
        }
        // No bytes or names are imported until the complete acknowledgement is representable.
        (request.preflight)(&files)?;
        let result = (|| {
            for (field, path, address, partition, identity, name_key) in imports {
                check_cancelled(cancellation)?;
                let mut staged = paths::TemporaryFile::new(&self.root)?;
                super::super::pack(
                    &path,
                    &mut CancelledWriter {
                        file: &mut staged.file,
                        cancellation,
                    },
                )
                .map_err(|error| {
                    std::io::Error::new(
                        error.kind(),
                        format!(
                            "artifacts.{} path {:?}: {error}",
                            field.as_str(),
                            selected[field]
                        ),
                    )
                })?;
                staged.file.rewind()?;
                let (blob, _) = self.import(&mut staged.file, &mut |entry| {
                    self.check_entry(
                        FileAuthorization {
                            access,
                            action: FileAction::Publish,
                            selection: &FileSelection {
                                address: address.clone(),
                                member: root_path(),
                            },
                        },
                        &partition,
                        entry,
                    )
                })?;
                catalog.trees.insert(
                    identity,
                    Tree {
                        partition,
                        address: address.clone(),
                        blob,
                        owner: Some(owner.clone()),
                    },
                );
                catalog.operations.insert(name_key, fingerprint.clone());
            }
            catalog.publications.insert(
                operation,
                Publication {
                    partition: publication_partition.unwrap_or_default(),
                    owner,
                    fingerprint,
                    published_at: now,
                    files: files.clone(),
                    released: false,
                },
            );
            check_cancelled(cancellation)?;
            self.save(&catalog)?;
            Ok(files)
        })();
        if result.is_err()
            && let Ok(committed) = self.read_catalog_locked()
        {
            // Save can fail after replacing the catalog. Reclaim only against committed
            // references, retaining the lock throughout and preserving the capture error.
            let _ = self.reclaim_locked(&committed);
        }
        result
    }

    /// Read a committed receipt without touching the workspace or creating a publication.
    pub fn publication_receipt(
        &self,
        access: &FileAccess<'_>,
        request: &super::super::publication::FilePublication,
    ) -> Result<Option<PublishedFiles>> {
        let (_lock, catalog) = self.load()?;
        self.publication_receipt_in(access, request, &catalog)
    }

    fn publication_receipt_in(
        &self,
        access: &FileAccess<'_>,
        request: &super::super::publication::FilePublication,
        catalog: &Catalog,
    ) -> Result<Option<PublishedFiles>> {
        let owner = &request.owner;
        let operation = self.operation_key(access, "capture-publication", owner)?;
        let fingerprint = key(&request.paths)?;
        if let Some(previous) = catalog.publications.get(&operation) {
            if &previous.owner != owner || previous.fingerprint != fingerprint || previous.released
            {
                return Err(invalid(
                    "publication identity conflicts or has been cleaned",
                ));
            }
            for file in previous.files.values() {
                if let PublishedFile::Managed { address, .. } = file {
                    let partition = self.selection(access, address, FileAction::Publish)?;
                    self.authorize_owner(
                        FileAuthorization {
                            access,
                            selection: &FileSelection {
                                address: address.clone(),
                                member: root_path(),
                            },
                            action: FileAction::Publish,
                        },
                        Some(owner),
                    )?;
                    if !catalog.trees.contains_key(&key(&(&partition, address))?) {
                        return Err(invalid("managed publication was released"));
                    }
                }
            }
            return Ok(Some(previous.files.clone()));
        }
        Ok(None)
    }

    pub fn select_publications(
        &self,
        _access: &FileAccess<'_>,
        now: UnixTimestampMillis,
        all: bool,
    ) -> Result<std::collections::BTreeSet<String>> {
        let (_lock, catalog) = self.load()?;
        Ok(catalog
            .publications
            .iter()
            .filter(|(_, publication)| {
                !publication.released
                    && (all
                        || now.get().saturating_sub(publication.published_at.get())
                            >= PUBLICATION_RETENTION_MILLIS)
            })
            .map(|(identity, _)| identity.clone())
            .collect())
    }

    /// Explicit host retention only: time, reads and startup never release publications.
    pub fn cleanup_publications(
        &self,
        access: &FileAccess<'_>,
        retention: &super::super::FileRetention<'_>,
    ) -> Result<usize> {
        let super::super::FileRetention {
            candidates,
            now,
            all,
            protected,
        } = *retention;
        let (_lock, mut catalog) = self.load()?;
        let mut removed = 0;
        for (identity, publication) in &mut catalog.publications {
            if !candidates.contains(identity)
                || protected.contains(publication.owner.run_id())
                || publication.released
                || (!all
                    && now.get().saturating_sub(publication.published_at.get())
                        < PUBLICATION_RETENTION_MILLIS)
            {
                continue;
            }
            let mut identities = Vec::new();
            for file in publication.files.values() {
                if let PublishedFile::Managed { address, .. } = file {
                    let partition = self.selection(access, address, FileAction::Publish)?;
                    self.authorize_owner(
                        FileAuthorization {
                            access,
                            action: FileAction::Publish,
                            selection: &FileSelection {
                                address: address.clone(),
                                member: root_path(),
                            },
                        },
                        Some(&publication.owner),
                    )?;
                    if partition != publication.partition {
                        return Err(denied());
                    }
                    identities.push(key(&(&partition, address))?);
                }
            }
            for identity in identities {
                catalog.trees.remove(&identity);
            }
            publication.released = true;
            removed += 1;
        }
        let uploader = self.operation_key(access, "upload-owner", &())?;
        for pin in catalog.pins.values_mut() {
            if pin.uploader.as_ref() == Some(&uploader)
                && pin.expires_at.is_some_and(|expires| all || now >= expires)
            {
                pin.blob = None;
            }
        }
        self.save(&catalog)?;
        self.reclaim_locked(&catalog)?;
        Ok(removed)
    }
}

fn check_cancelled(cancellation: &crate::execution::driver::DriverCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
    }
    Ok(())
}
struct CancelledWriter<'a> {
    file: &'a mut File,
    cancellation: &'a crate::execution::driver::DriverCancellation,
}
impl Write for CancelledWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        check_cancelled(self.cancellation)?;
        self.file.write(bytes)
    }
    fn flush(&mut self) -> Result<()> {
        check_cancelled(self.cancellation)?;
        self.file.flush()
    }
}
