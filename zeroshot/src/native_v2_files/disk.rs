use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs2::FileExt;
use openengine_cluster_protocol::{
    FileAddress, FileEntry, FilePath, FileScope, FileSelection, MAX_FILE_METADATA_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod publications;

use super::{
    FileAccess, FileAction, FileAuthorization, FileAuthorizer, FilePin, FilePinRequest,
    FilePublicationOwner, FilePublishRequest, FileReplaceRequest, HostFilePublication, Result,
    TargetFileHost, denied, invalid, paths, transfer,
};

#[derive(Clone)]
pub struct DiskFileStore {
    root: paths::Directory,
    target: String,
    authorization: Arc<dyn FileAuthorizer>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    target: String,
    trees: BTreeMap<String, Tree>,
    pins: BTreeMap<String, Pin>,
    operations: BTreeMap<String, String>,
    #[serde(default)]
    publications: BTreeMap<String, publications::Publication>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Tree {
    partition: String,
    address: FileAddress,
    blob: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<FilePublicationOwner>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<FilePublicationOwner>,
    partition: String,
    source: FileSelection,
    staged: bool,
    blob: Option<String>,
    digest: String,
}

fn key(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn root_path() -> FilePath {
    FilePath::new("").expect("empty root is portable")
}

impl DiskFileStore {
    pub fn open(
        root: PathBuf,
        target: String,
        authorization: Arc<dyn FileAuthorizer>,
    ) -> Result<Self> {
        if target.is_empty() {
            return Err(invalid("file target identity is required"));
        }
        let root = std::path::absolute(root)?;
        paths::private_root(&root)?;
        let root = paths::Directory::open(&root)?;
        match root.create_directory(std::ffi::OsStr::new("objects")) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        root.child(std::ffi::OsStr::new("objects"))?
            .validate_private()?;
        let store = Self {
            root,
            target,
            authorization,
        };
        let (_lock, catalog) = store.load()?;
        if catalog.target != store.target {
            return Err(denied());
        }
        store.save(&catalog)?;
        store.reclaim_locked(&catalog)?;
        Ok(store)
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    fn authorize(
        &self,
        access: &FileAccess<'_>,
        selection: &FileSelection,
        action: FileAction,
    ) -> Result<String> {
        if access.target != self.target {
            return Err(denied());
        }
        self.authorization.authorize(FileAuthorization {
            access,
            action,
            selection,
        })
    }

    fn selection(
        &self,
        access: &FileAccess<'_>,
        address: &FileAddress,
        action: FileAction,
    ) -> Result<String> {
        if address.path.as_str().is_empty() {
            return Err(invalid("named file path must not be empty"));
        }
        let mut selected = FileSelection {
            address: FileAddress {
                scope: address.scope.clone(),
                path: root_path(),
            },
            member: root_path(),
        };
        let partition = self.authorize(access, &selected, action)?;
        let mut prefix = String::new();
        for component in address.path.as_str().split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            selected.address.path = FilePath::new(&prefix).map_err(invalid)?;
            if self.authorize(access, &selected, action)? != partition {
                return Err(denied());
            }
        }
        Ok(partition)
    }

    fn member_authority(
        &self,
        access: &FileAccess<'_>,
        selection: &FileSelection,
        action: FileAction,
    ) -> Result<String> {
        let partition = self.selection(access, &selection.address, action)?;
        let mut member = String::new();
        for component in selection
            .member
            .as_str()
            .split('/')
            .filter(|s| !s.is_empty())
        {
            if !member.is_empty() {
                member.push('/');
            }
            member.push_str(component);
            let selected = FileSelection {
                address: selection.address.clone(),
                member: FilePath::new(&member).map_err(invalid)?,
            };
            if self.authorize(access, &selected, action)? != partition {
                return Err(denied());
            }
        }
        Ok(partition)
    }

    fn check_entry(
        &self,
        request: FileAuthorization<'_>,
        partition: &str,
        entry: &FileEntry,
    ) -> Result<()> {
        let selection = FileSelection {
            address: request.selection.address.clone(),
            member: entry.path.clone(),
        };
        if self.authorize(request.access, &selection, request.action)? != partition {
            return Err(denied());
        }
        Ok(())
    }

    fn authorize_owner(
        &self,
        request: FileAuthorization<'_>,
        owner: Option<&FilePublicationOwner>,
    ) -> Result<()> {
        if let Some(owner) = owner {
            self.authorization.authorize_publication(request, owner)?;
        }
        Ok(())
    }

    fn load(&self) -> Result<(File, Catalog)> {
        self.root.validate_private()?;
        let lock = self
            .root
            .writable_file(std::ffi::OsStr::new("catalog.lock"))?;
        paths::regular(&lock)?;
        lock.lock_exclusive()?;
        let catalog = match self.root.read_file(std::ffi::OsStr::new("catalog.json")) {
            Ok(file) => {
                paths::regular(&file)?;
                let mut bytes = Vec::new();
                file.take(MAX_FILE_METADATA_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > MAX_FILE_METADATA_BYTES {
                    return Err(invalid("file catalog exceeds limit"));
                }
                serde_json::from_slice::<Catalog>(&bytes)
                    .map_err(|_| invalid("invalid file catalog"))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Catalog {
                target: self.target.clone(),
                ..Catalog::default()
            },
            Err(error) => return Err(error),
        };
        if catalog.target != self.target {
            return Err(denied());
        }
        Ok((lock, catalog))
    }

    fn save(&self, catalog: &Catalog) -> Result<()> {
        let bytes = serde_json::to_vec(catalog)?;
        if bytes.len() > MAX_FILE_METADATA_BYTES {
            return Err(invalid("file catalog exceeds limit"));
        }
        let mut staged = paths::TemporaryFile::new(&self.root)?;
        staged.file.write_all(&bytes)?;
        staged.commit(std::ffi::OsStr::new("catalog.json"))
    }

    fn import(
        &self,
        input: &mut dyn Read,
        authorize: &mut dyn FnMut(&FileEntry) -> Result<()>,
    ) -> Result<(String, String)> {
        let objects = self.root.child(std::ffi::OsStr::new("objects"))?;
        objects.validate_private()?;
        let mut staged = paths::TemporaryFile::new(&objects)?;
        transfer::copy_validated(input, &mut staged.file, authorize)?;
        staged.file.sync_all()?;
        staged.file.rewind()?;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = staged.file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let digest = format!("{:x}", digest.finalize());
        let id = uuid::Uuid::now_v7().to_string();
        // Persist before catalog publication. A crash or ambiguous catalog sync leaves an orphan
        // for reclaim, never a catalog entry pointing at a deleted object.
        staged.commit(std::ffi::OsStr::new(&id))?;
        Ok((id, digest))
    }

    fn object(&self, blob: &str) -> Result<File> {
        if uuid::Uuid::parse_str(blob).is_err() || blob.contains(['/', '\\']) {
            return Err(invalid("invalid stored object identity"));
        }
        let objects = self.root.child(std::ffi::OsStr::new("objects"))?;
        objects.validate_private()?;
        let file = objects.read_file(std::ffi::OsStr::new(blob))?;
        paths::regular(&file)?;
        Ok(file)
    }

    fn operation_key(
        &self,
        access: &FileAccess<'_>,
        kind: &str,
        operation: &impl Serialize,
    ) -> Result<String> {
        if access.target != self.target {
            return Err(denied());
        }
        key(&(
            access.identity.tenant().as_str(),
            access.identity.principal().as_str(),
            kind,
            operation,
        ))
    }

    fn pin_key(&self, access: &FileAccess<'_>, pin: &FilePin) -> Result<String> {
        self.operation_key(access, "pin", pin)
    }

    fn staged_source(pin: &FilePin) -> Result<FileSelection> {
        Ok(FileSelection {
            address: FileAddress {
                scope: FileScope::Run {
                    run_id: pin.run_id.clone(),
                },
                path: FilePath::new(format!("inputs/{}", key(&pin.operation)?)).map_err(invalid)?,
            },
            member: root_path(),
        })
    }

    fn retained_pin<'a>(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        catalog: &'a Catalog,
    ) -> Result<&'a Pin> {
        let retained = catalog
            .pins
            .get(&self.pin_key(access, pin)?)
            .ok_or_else(denied)?;
        self.authorize_owner(
            FileAuthorization {
                access,
                selection: &retained.source,
                action: FileAction::Pin,
            },
            retained.owner.as_ref(),
        )?;
        if retained.blob.is_none() {
            return Err(invalid("file pin has been released"));
        }
        if self.member_authority(access, &retained.source, FileAction::Pin)? != retained.partition {
            return Err(denied());
        }
        // A reusable source alone never grants ownership of a run's private pin.
        self.member_authority(access, &Self::staged_source(pin)?, FileAction::Pin)?;
        Ok(retained)
    }

    fn reclaim_locked(&self, catalog: &Catalog) -> Result<usize> {
        let live: BTreeSet<_> = catalog
            .trees
            .values()
            .map(|tree| tree.blob.as_str())
            .chain(catalog.pins.values().filter_map(|pin| pin.blob.as_deref()))
            .collect();
        let mut count = 0;
        let objects = self.root.child(std::ffi::OsStr::new("objects"))?;
        objects.validate_private()?;
        for (directory, object_directory) in [(&self.root, false), (&objects, true)] {
            for name in directory.entries()? {
                let name = name?;
                let name = name
                    .to_str()
                    .ok_or_else(|| invalid("invalid file store entry"))?;
                let orphan =
                    object_directory && uuid::Uuid::parse_str(name).is_ok() && !live.contains(name);
                if name.starts_with("stage-") || orphan {
                    // Never recurse through unexpected paths or links.
                    let file = directory.read_file(std::ffi::OsStr::new(name))?;
                    paths::regular(&file)?;
                    drop(file);
                    directory.remove_file(std::ffi::OsStr::new(name))?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

impl TargetFileHost for DiskFileStore {
    fn list(
        &self,
        access: &FileAccess<'_>,
        scope: FileScope,
        prefix: &FilePath,
    ) -> Result<Vec<FileAddress>> {
        let selected = FileSelection {
            address: FileAddress {
                scope: scope.clone(),
                path: prefix.clone(),
            },
            member: root_path(),
        };
        let partition = if prefix.as_str().is_empty() {
            self.authorize(access, &selected, FileAction::List)?
        } else {
            self.selection(access, &selected.address, FileAction::List)?
        };
        let (_lock, catalog) = self.load()?;
        let mut paths = Vec::new();
        for tree in catalog.trees.values() {
            if tree.partition == partition
                && tree.address.scope == scope
                && (prefix.as_str().is_empty()
                    || tree.address.path == *prefix
                    || tree
                        .address
                        .path
                        .as_str()
                        .starts_with(&format!("{}/", prefix.as_str())))
            {
                let selection = FileSelection {
                    address: tree.address.clone(),
                    member: root_path(),
                };
                if self
                    .authorize_owner(
                        FileAuthorization {
                            access,
                            selection: &selection,
                            action: FileAction::List,
                        },
                        tree.owner.as_ref(),
                    )
                    .is_err()
                {
                    continue;
                }
                if self.selection(access, &tree.address, FileAction::List)? != partition {
                    return Err(denied());
                }
                paths.push(tree.address.clone());
            }
        }
        paths.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(paths)
    }

    fn list_members(
        &self,
        access: &FileAccess<'_>,
        selection: &FileSelection,
    ) -> Result<Vec<FileEntry>> {
        let partition = self.member_authority(access, selection, FileAction::List)?;
        let (_lock, catalog) = self.load()?;
        let tree = catalog
            .trees
            .get(&key(&(&partition, &selection.address))?)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "file path was not found")
            })?;
        self.authorize_owner(
            FileAuthorization {
                access,
                selection,
                action: FileAction::List,
            },
            tree.owner.as_ref(),
        )?;
        transfer::members(
            &mut self.object(&tree.blob)?,
            &selection.member,
            &mut |entry| {
                self.check_entry(
                    FileAuthorization {
                        access,
                        action: FileAction::List,
                        selection,
                    },
                    &partition,
                    entry,
                )
            },
        )
    }

    fn replace(
        &self,
        access: &FileAccess<'_>,
        request: &FileReplaceRequest,
        input: &mut dyn Read,
    ) -> Result<()> {
        if matches!(request.address.scope, FileScope::Run { .. }) {
            return Err(denied());
        }
        let partition = self.selection(access, &request.address, FileAction::Replace)?;
        let (_lock, mut catalog) = self.load()?;
        self.reclaim_locked(&catalog)?;
        if let Some(previous) = catalog.trees.get(&key(&(&partition, &request.address))?) {
            transfer::copy_validated(
                &mut self.object(&previous.blob)?,
                &mut std::io::sink(),
                &mut |entry| {
                    self.check_entry(
                        FileAuthorization {
                            access,
                            action: FileAction::Replace,
                            selection: &FileSelection {
                                address: request.address.clone(),
                                member: root_path(),
                            },
                        },
                        &partition,
                        entry,
                    )
                },
            )?;
        }
        let (blob, digest) = self.import(input, &mut |entry| {
            self.check_entry(
                FileAuthorization {
                    access,
                    action: FileAction::Replace,
                    selection: &FileSelection {
                        address: request.address.clone(),
                        member: root_path(),
                    },
                },
                &partition,
                entry,
            )
        })?;
        let operation = self.operation_key(access, "replace", &request.operation)?;
        let fingerprint = key(&(&partition, &request.address, digest))?;
        if let Some(previous) = catalog.operations.get(&operation) {
            if *previous != fingerprint {
                return Err(invalid(
                    "file operation key was reused with different input",
                ));
            }
            return Ok(());
        }
        catalog.trees.insert(
            key(&(&partition, &request.address))?,
            Tree {
                owner: None,
                partition,
                address: request.address.clone(),
                blob,
            },
        );
        catalog.operations.insert(operation, fingerprint);
        self.save(&catalog)
    }

    fn download(
        &self,
        access: &FileAccess<'_>,
        selection: &FileSelection,
        output: &mut dyn Write,
    ) -> Result<()> {
        let partition = self.member_authority(access, selection, FileAction::Read)?;
        let (_lock, catalog) = self.load()?;
        let tree = catalog
            .trees
            .get(&key(&(&partition, &selection.address))?)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "file path was not found")
            })?;
        self.authorize_owner(
            FileAuthorization {
                access,
                selection,
                action: FileAction::Read,
            },
            tree.owner.as_ref(),
        )?;
        transfer::project(
            &mut self.object(&tree.blob)?,
            &selection.member,
            output,
            &mut |entry| {
                self.check_entry(
                    FileAuthorization {
                        access,
                        action: FileAction::Read,
                        selection: &FileSelection {
                            address: selection.address.clone(),
                            member: root_path(),
                        },
                    },
                    &partition,
                    entry,
                )
            },
        )
    }

    fn remove(&self, access: &FileAccess<'_>, address: &FileAddress) -> Result<bool> {
        if matches!(address.scope, FileScope::Run { .. }) {
            return Err(denied());
        }
        let partition = self.selection(access, address, FileAction::Remove)?;
        let (_lock, mut catalog) = self.load()?;
        let identity = key(&(&partition, address))?;
        if let Some(tree) = catalog.trees.get(&identity) {
            // Whole-tree effects require authority over every member, including directories.
            transfer::copy_validated(
                &mut self.object(&tree.blob)?,
                &mut std::io::sink(),
                &mut |entry| {
                    self.check_entry(
                        FileAuthorization {
                            access,
                            action: FileAction::Remove,
                            selection: &FileSelection {
                                address: address.clone(),
                                member: root_path(),
                            },
                        },
                        &partition,
                        entry,
                    )
                },
            )?;
        }
        let removed = catalog.trees.remove(&identity).is_some();
        self.save(&catalog)?;
        self.reclaim_locked(&catalog)?;
        Ok(removed)
    }

    fn pin(&self, access: &FileAccess<'_>, request: &FilePinRequest) -> Result<FilePin> {
        let partition = self.member_authority(access, &request.source, FileAction::Pin)?;
        self.member_authority(access, &Self::staged_source(&request.pin)?, FileAction::Pin)?;
        let (_lock, mut catalog) = self.load()?;
        let identity = self.pin_key(access, &request.pin)?;
        if let Some(previous) = catalog.pins.get(&identity) {
            if previous.partition != partition
                || previous.source != request.source
                || previous.staged
                || previous.blob.is_none()
            {
                return Err(invalid("file pin operation conflicts or was released"));
            }
            return Ok(request.pin.clone());
        }
        let tree = catalog
            .trees
            .get(&key(&(&partition, &request.source.address))?)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "file path was not found")
            })?;
        transfer::project(
            &mut self.object(&tree.blob)?,
            &request.source.member,
            &mut std::io::sink(),
            &mut |entry| {
                self.check_entry(
                    FileAuthorization {
                        access,
                        action: FileAction::Pin,
                        selection: &FileSelection {
                            address: request.source.address.clone(),
                            member: root_path(),
                        },
                    },
                    &partition,
                    entry,
                )
            },
        )?;
        self.authorize_owner(
            FileAuthorization {
                access,
                selection: &request.source,
                action: FileAction::Pin,
            },
            tree.owner.as_ref(),
        )?;
        let owner = tree.owner.clone();
        let blob = tree.blob.clone();
        catalog.pins.insert(
            identity,
            Pin {
                owner,
                partition,
                source: request.source.clone(),
                staged: false,
                blob: Some(blob),
                digest: String::new(),
            },
        );
        self.save(&catalog)?;
        Ok(request.pin.clone())
    }

    fn stage(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        input: &mut dyn Read,
    ) -> Result<FilePin> {
        let source = Self::staged_source(pin)?;
        let partition = self.member_authority(access, &source, FileAction::Pin)?;
        let (_lock, mut catalog) = self.load()?;
        let identity = self.pin_key(access, pin)?;
        if catalog.pins.get(&identity).is_some_and(|previous| {
            previous.partition != partition || !previous.staged || previous.blob.is_none()
        }) {
            return Err(invalid("file stage operation conflicts or was released"));
        }
        self.reclaim_locked(&catalog)?;
        let (blob, digest) = self.import(input, &mut |entry| {
            self.check_entry(
                FileAuthorization {
                    access,
                    action: FileAction::Pin,
                    selection: &FileSelection {
                        address: source.address.clone(),
                        member: root_path(),
                    },
                },
                &partition,
                entry,
            )
        })?;
        if let Some(previous) = catalog.pins.get(&identity) {
            if previous.digest != digest {
                return Err(invalid("file stage operation conflicts or was released"));
            }
            return Ok(pin.clone());
        }
        catalog.pins.insert(
            identity,
            Pin {
                owner: None,
                partition,
                source,
                staged: true,
                blob: Some(blob),
                digest,
            },
        );
        self.save(&catalog)?;
        Ok(pin.clone())
    }

    fn read_pin(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        output: &mut dyn Write,
    ) -> Result<()> {
        let (_lock, catalog) = self.load()?;
        let retained = self.retained_pin(access, pin, &catalog)?;
        let partition = self.member_authority(access, &retained.source, FileAction::Pin)?;
        transfer::project(
            &mut self.object(retained.blob.as_deref().ok_or_else(denied)?)?,
            &retained.source.member,
            output,
            &mut |entry| {
                self.check_entry(
                    FileAuthorization {
                        access,
                        action: FileAction::Pin,
                        selection: &FileSelection {
                            address: retained.source.address.clone(),
                            member: root_path(),
                        },
                    },
                    &partition,
                    entry,
                )
            },
        )
    }

    fn materialize(
        &self,
        access: &FileAccess<'_>,
        pin: &FilePin,
        destination: &Path,
    ) -> Result<()> {
        let (_lock, catalog) = self.load()?;
        let retained = self.retained_pin(access, pin, &catalog)?;
        let partition = self.member_authority(access, &retained.source, FileAction::Pin)?;
        let mut staged = paths::TemporaryFile::new(&self.root)?;
        transfer::project(
            &mut self.object(retained.blob.as_deref().ok_or_else(denied)?)?,
            &retained.source.member,
            &mut staged.file,
            &mut |entry| {
                self.check_entry(
                    FileAuthorization {
                        access,
                        action: FileAction::Pin,
                        selection: &retained.source,
                    },
                    &partition,
                    entry,
                )
            },
        )?;
        super::unpack(&mut staged.file, destination)
    }

    fn publish(
        &self,
        access: &FileAccess<'_>,
        request: &FilePublishRequest,
    ) -> Result<HostFilePublication> {
        if request.name.as_str().is_empty() || request.pin.run_id != *request.owner.run_id() {
            return Err(invalid("publication must belong to its pinned run"));
        }
        let prefix = match &request.owner {
            FilePublicationOwner::Request { request, .. } => format!("requests/{}", key(request)?),
            FilePublicationOwner::Final { .. } => "final".to_owned(),
        };
        let address = FileAddress {
            scope: FileScope::Run {
                run_id: request.owner.run_id().clone(),
            },
            path: FilePath::new(format!("{prefix}/{}", request.name.as_str())).map_err(invalid)?,
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
            Some(&request.owner),
        )?;
        let (_lock, mut catalog) = self.load()?;
        let operation =
            self.operation_key(access, "publish", &(&request.owner, &request.operation))?;
        let name = key(&("publication-name", &partition, &address))?;
        let fingerprint = key(&(&address, &request.pin))?;
        if let Some(previous) = catalog.operations.get(&operation) {
            if *previous != fingerprint {
                return Err(invalid("publication operation conflicts"));
            }
            if !catalog.trees.contains_key(&key(&(&partition, &address))?) {
                return Err(invalid("managed publication was released"));
            }
            return Ok(HostFilePublication::Managed { address });
        }
        // A released receipt must never resolve to another publication's bytes after recovery.
        if catalog.operations.contains_key(&name) {
            return Err(invalid("managed publication name already used"));
        }
        let retained = self.retained_pin(access, &request.pin, &catalog)?;
        // Project subtrees into their own managed object, retaining only the selected members.
        let mut selected = paths::TemporaryFile::new(&self.root)?;
        let source_partition = self.member_authority(access, &retained.source, FileAction::Pin)?;
        transfer::project(
            &mut self.object(retained.blob.as_deref().ok_or_else(denied)?)?,
            &retained.source.member,
            &mut selected.file,
            &mut |entry| {
                self.check_entry(
                    FileAuthorization {
                        access,
                        action: FileAction::Pin,
                        selection: &FileSelection {
                            address: retained.source.address.clone(),
                            member: root_path(),
                        },
                    },
                    &source_partition,
                    entry,
                )
            },
        )?;
        selected.file.rewind()?;
        let (blob, _) = self.import(&mut selected.file, &mut |entry| {
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
        let identity = key(&(&partition, &address))?;
        if catalog.trees.contains_key(&identity) {
            return Err(invalid("managed publication name already exists"));
        }
        catalog.trees.insert(
            identity,
            Tree {
                owner: Some(request.owner.clone()),
                partition: partition.clone(),
                address: address.clone(),
                blob,
            },
        );
        catalog.operations.insert(name, fingerprint.clone());
        catalog
            .operations
            .insert(operation.clone(), fingerprint.clone());
        catalog.publications.insert(
            operation,
            publications::Publication {
                partition,
                owner: request.owner.clone(),
                fingerprint,
                published_at: crate::native_v2_runner::current_timestamp(),
                released: false,
                files: [(
                    openengine_cluster_protocol::FieldName::new("file")
                        .map_err(|_| invalid("invalid artifact name"))?,
                    openengine_cluster_protocol::PublishedFile::Managed {
                        target: access.target.to_owned(),
                        address: address.clone(),
                    },
                )]
                .into_iter()
                .collect(),
            },
        );
        self.save(&catalog)?;
        Ok(HostFilePublication::Managed { address })
    }

    fn release_publication(&self, access: &FileAccess<'_>, address: &FileAddress) -> Result<bool> {
        if !matches!(address.scope, FileScope::Run { .. }) {
            return Err(invalid("managed run publication required"));
        }
        let partition = self.selection(access, address, FileAction::Publish)?;
        let (_lock, mut catalog) = self.load()?;
        let identity = key(&(&partition, address))?;
        if let Some(tree) = catalog.trees.get(&identity) {
            self.authorize_owner(
                FileAuthorization {
                    access,
                    action: FileAction::Publish,
                    selection: &FileSelection {
                        address: address.clone(),
                        member: root_path(),
                    },
                },
                tree.owner.as_ref(),
            )?;
            transfer::copy_validated(
                &mut self.object(&tree.blob)?,
                &mut std::io::sink(),
                &mut |entry| {
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
                },
            )?;
        }
        let removed = catalog.trees.remove(&identity).is_some();
        catalog
            .operations
            .entry(key(&("publication-name", &partition, address))?)
            .or_default();
        self.save(&catalog)?;
        self.reclaim_locked(&catalog)?;
        Ok(removed)
    }

    fn release(&self, access: &FileAccess<'_>, pin: &FilePin) -> Result<()> {
        let source = Self::staged_source(pin)?;
        let partition = self.member_authority(access, &source, FileAction::Pin)?;
        let (_lock, mut catalog) = self.load()?;
        catalog
            .pins
            .entry(self.pin_key(access, pin)?)
            .or_insert(Pin {
                owner: None,
                partition,
                source,
                staged: true,
                blob: None,
                digest: String::new(),
            })
            .blob = None;
        self.save(&catalog)?;
        self.reclaim_locked(&catalog)?;
        Ok(())
    }

    fn select_publications(
        &self,
        access: &FileAccess<'_>,
        now: openengine_cluster_protocol::UnixTimestampMillis,
        all: bool,
    ) -> Result<std::collections::BTreeSet<String>> {
        DiskFileStore::select_publications(self, access, now, all)
    }
    fn cleanup_publications(
        &self,
        access: &FileAccess<'_>,
        retention: &super::FileRetention<'_>,
    ) -> Result<usize> {
        DiskFileStore::cleanup_publications(self, access, retention)
    }

    fn reclaim(&self) -> Result<usize> {
        let (_lock, catalog) = self.load()?;
        self.reclaim_locked(&catalog)
    }
}
