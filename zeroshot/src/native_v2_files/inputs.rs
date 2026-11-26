//! Private filepath admission shared by submissions and request answers.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Seek};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use openengine_cluster_protocol::{IdempotencyKey, PayloadType, RunFileInput, RunId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{FileAccess, FilePin, FilePinRequest, WorkspaceFiles, invalid, paths};

/// Only local callers supply a local root. Remote strings never grant host filesystem authority.
pub struct InputAdmission<'a> {
    pub run_id: &'a RunId,
    pub owner: &'a RunId,
    pub key: &'a IdempotencyKey,
    pub schema: &'a PayloadType,
    pub value: &'a Value,
    pub local_root: Option<&'a Path>,
}

/// Only the attempt that first froze a manifest may roll it back before admission.
#[derive(Debug)]
pub struct FileInputClaim {
    run_id: RunId,
    pin_nonce: String,
}

#[derive(Debug)]
pub struct FrozenFileInputs {
    pub run_id: RunId,
    pub value: Value,
    pub claim: Option<FileInputClaim>,
}

/// Only declared filepath fields can select temporary uploads for reclamation.
pub fn staged_uploads(schema: &PayloadType, value: &Value) -> io::Result<Vec<FilePin>> {
    let mut uploads = Vec::new();
    visit(schema, &mut value.clone(), "", &mut |_, value| {
        if let Ok(RunFileInput::Staged { run_id, operation }) =
            serde_json::from_value(value.clone())
        {
            uploads.push(FilePin { run_id, operation });
        }
        Ok(())
    })
    .map_err(io::Error::other)?;
    Ok(uploads)
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    Target {
        selection: openengine_cluster_protocol::FileSelection,
    },
    Staged {
        pin: FilePin,
    },
    Local {
        path: PathBuf,
    },
}
#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    run_id: RunId,
    owner: RunId,
    key: IdempotencyKey,
    sources: BTreeMap<String, Source>,
    pin_nonce: String,
}

fn hash(value: &impl Serialize) -> io::Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

// Private input paths contain three nested identifiers. Keep each at 128 bits so ordinary
// Windows state directories stay below MAX_PATH without weakening manifest or pin identities.
fn path_hash(value: &impl Serialize) -> io::Result<String> {
    Ok(hash(value)?[..32].to_owned())
}
fn pin(manifest: &Manifest, pointer: &str) -> io::Result<FilePin> {
    Ok(FilePin {
        run_id: manifest.run_id.clone(),
        operation: IdempotencyKey::new(format!("input-{}", hash(&(&manifest.pin_nonce, pointer))?))
            .map_err(|_| invalid("invalid input pin"))?,
    })
}
fn write(directory: &paths::Directory, name: &str, value: &impl Serialize) -> io::Result<()> {
    let mut file = paths::TemporaryFile::new(directory)?;
    serde_json::to_writer(&mut file.file, value)?;
    file.commit(std::ffi::OsStr::new(name))
}

fn released(directory: &paths::Directory, run_id: &RunId) -> io::Result<bool> {
    match directory.read_file(std::ffi::OsStr::new(&format!("released-{}", hash(run_id)?))) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Pin the complete parent ancestry before deleting an input tree. The final name is
/// removed relative to that capability, so a redirected lineage cannot select another tree.
pub(crate) fn remove_input_tree(path: &Path) -> io::Result<()> {
    let result = (|| {
        let parent = path
            .parent()
            .ok_or_else(|| invalid("input root has no parent"))?;
        let name = path
            .file_name()
            .ok_or_else(|| invalid("input root has no name"))?;
        paths::Directory::open(parent)?.remove_tree(name)
    })();
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn bind_file_inputs(
    directory: &paths::Directory,
    request: &InputAdmission<'_>,
    sources: BTreeMap<String, Source>,
    answer: bool,
) -> io::Result<(Manifest, bool)> {
    let name = format!("submission-{}.json", hash(request.key)?);
    let expected = Manifest {
        run_id: request.run_id.clone(),
        owner: request.owner.clone(),
        key: request.key.clone(),
        sources,
        pin_nonce: uuid::Uuid::now_v7().to_string(),
    };
    let mut manifest = match directory.read_file(std::ffi::OsStr::new(&name)) {
        Ok(file) => {
            let manifest: Manifest = serde_json::from_reader(file)?;
            if manifest.key != expected.key || manifest.sources != expected.sources {
                return Err(invalid("submission key identifies different file inputs"));
            }
            if answer && (manifest.run_id != expected.run_id || manifest.owner != expected.owner) {
                return Err(invalid(
                    "answer file inputs belong to another successor or lineage",
                ));
            }
            manifest
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => expected,
        Err(error) => return Err(error),
    };
    let run_name = format!("run-{}.json", hash(&manifest.run_id)?);
    let created = match directory.read_file(std::ffi::OsStr::new(&run_name)) {
        Ok(file) => {
            let recorded: Manifest = serde_json::from_reader(file)?;
            if recorded.run_id != manifest.run_id
                || recorded.owner != manifest.owner
                || recorded.key != manifest.key
                || recorded.sources != manifest.sources
            {
                return Err(invalid("run ID identifies different file inputs"));
            }
            // Recover a run manifest committed just before its submission-key manifest.
            manifest = recorded;
            false
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => return Err(error),
    };
    // Bind the submission key first so a retry with a fresh proposed ID finds the same owner.
    write(directory, &name, &manifest)?;
    write(directory, &run_name, &manifest)?;
    if released(directory, &manifest.run_id)? {
        return Err(invalid(
            "file inputs were released; use a new submission key",
        ));
    }
    Ok((manifest, created))
}

/// Validate the ordinary value after replacing only filepath selectors with a path placeholder.
/// Optional fields and undeclared keys remain the schema validator's responsibility.
pub fn validate_input(schema: &PayloadType, value: &Value) -> Result<(), String> {
    let mut value = value.clone();
    visit(schema, &mut value, "", &mut |_, value| {
        if !value.is_string() {
            serde_json::from_value::<RunFileInput>(value.clone())
                .map_err(|_| "filepath requires a target selection or private upload".to_owned())?;
            *value = Value::String("run-owned-input".into());
        }
        Ok(())
    })?;
    schema
        .validate_value(&value)
        .map_err(|error| error.to_string())
}

pub fn has_file_inputs(schema: &PayloadType, value: &Value) -> bool {
    let mut found = false;
    let _ = visit(schema, &mut value.clone(), "", &mut |_, _| {
        found = true;
        Ok(())
    });
    found
}

pub(crate) fn visit(
    schema: &PayloadType,
    value: &mut Value,
    pointer: &str,
    action: &mut impl FnMut(&str, &mut Value) -> Result<(), String>,
) -> Result<(), String> {
    match schema {
        PayloadType::Filepath => action(pointer, value)?,
        PayloadType::Record { fields } => {
            if let Some(object) = value.as_object_mut() {
                for (name, field) in fields {
                    if let Some(value) = object.get_mut(name.as_str()) {
                        let escaped = name.as_str().replace('~', "~0").replace('/', "~1");
                        visit(
                            &field.value_type,
                            value,
                            &format!("{pointer}/{escaped}"),
                            action,
                        )?;
                    }
                }
            }
        }
        PayloadType::Array { items } => {
            if let Some(array) = value.as_array_mut() {
                for (index, value) in array.iter_mut().enumerate() {
                    visit(items, value, &format!("{pointer}/{index}"), action)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

impl WorkspaceFiles {
    fn input_directory(&self) -> io::Result<paths::Directory> {
        let path = self.directory.join("file-inputs");
        paths::private_root(&path)?;
        let directory = paths::Directory::open(&path)?;
        directory.validate_private()?;
        Ok(directory)
    }
    fn input_access(&self) -> FileAccess<'_> {
        FileAccess {
            target: &self.target,
            identity: &self.identity,
        }
    }
    pub fn input_data_directory(&self) -> PathBuf {
        self.directory.with_extension("input-data")
    }
    fn prepare_input_directory(root: &Path) -> io::Result<()> {
        paths::reject_links(root)?;
        if !root.exists() {
            crate::execution::platform::private_directory(root)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(root)?;
            // Hosted writers need traversal, while only the controller may list or change names.
            if !metadata.is_dir()
                || metadata.mode() & 0o022 != 0
                || metadata.uid() != {
                    // SAFETY: geteuid has no preconditions.
                    unsafe { libc::geteuid() }
                }
            {
                return Err(super::denied());
            }
        }
        #[cfg(windows)]
        crate::execution::platform::ensure_private_directory(root)?;
        Ok(())
    }

    pub fn input_location(&self, owner: &RunId) -> io::Result<PathBuf> {
        Ok(std::path::absolute(self.input_data_directory())?.join(path_hash(owner)?))
    }

    fn input_root(&self, owner: &RunId, key: &IdempotencyKey) -> io::Result<PathBuf> {
        Ok(self.input_location(owner)?.join(path_hash(key)?))
    }

    fn project_file_inputs(
        &self,
        request: &InputAdmission<'_>,
    ) -> io::Result<(Value, BTreeMap<String, Source>)> {
        if !has_file_inputs(request.schema, request.value) {
            return Ok((request.value.clone(), BTreeMap::new()));
        }
        validate_input(request.schema, request.value).map_err(io::Error::other)?;
        let mut value = request.value.clone();
        let mut sources = BTreeMap::new();
        visit(request.schema, &mut value, "", &mut |pointer, value| {
            let source = match value.as_str() {
                Some(path) => {
                    let root = request.local_root.ok_or(
                        "filepath requires an authorized target selection or private upload",
                    )?;
                    Source::Local {
                        path: root.join(path),
                    }
                }
                None => {
                    match serde_json::from_value(value.clone()).map_err(|_| "invalid file input")? {
                        RunFileInput::Target { selection } => Source::Target { selection },
                        RunFileInput::Staged { run_id, operation } => Source::Staged {
                            pin: FilePin { run_id, operation },
                        },
                    }
                }
            };
            sources.insert(pointer.to_owned(), source);
            Ok(())
        })
        .map_err(io::Error::other)?;
        if sources.is_empty() {
            return Ok((value, sources));
        }
        // A retry may propose a fresh run ID. Its projection must retain the first owner's paths.
        let directory = self.input_directory()?;
        let recorded = match directory.read_file(std::ffi::OsStr::new(&format!(
            "submission-{}.json",
            hash(request.key)?
        ))) {
            Ok(file) => Some(serde_json::from_reader::<_, Manifest>(file)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let owner = recorded
            .as_ref()
            .map_or(request.owner, |manifest| &manifest.owner);
        let root = self.input_root(owner, request.key)?;
        for (pointer, source) in &sources {
            *value
                .pointer_mut(pointer)
                .ok_or_else(|| invalid("file input field disappeared"))? = Value::String(
                root.join(path_hash(&(pointer, source))?)
                    .to_str()
                    .ok_or_else(|| invalid("input path is not UTF-8"))?
                    .to_owned(),
            );
        }
        Ok((value, sources))
    }

    pub fn project_input_value(&self, request: &InputAdmission<'_>) -> io::Result<Value> {
        self.project_file_inputs(request).map(|(value, _)| value)
    }

    /// Freeze every explicit file, including local files already inside a live checkout.
    /// One lock binds both identities before any manifest or private pin can change.
    pub fn freeze_file_inputs(&self, request: InputAdmission<'_>) -> io::Result<FrozenFileInputs> {
        self.freeze_explicit_inputs(request, false)
    }

    /// Answers retain the exact successor and resolved recovery-lineage identities on replay.
    pub fn freeze_answer_inputs(
        &self,
        request: InputAdmission<'_>,
    ) -> io::Result<FrozenFileInputs> {
        self.freeze_explicit_inputs(request, true)
    }

    fn freeze_explicit_inputs(
        &self,
        request: InputAdmission<'_>,
        answer: bool,
    ) -> io::Result<FrozenFileInputs> {
        let (mut value, sources) = self.project_file_inputs(&request)?;
        if sources.is_empty() {
            return Ok(FrozenFileInputs {
                run_id: request.run_id.clone(),
                value,
                claim: None,
            });
        }
        let directory = self.input_directory()?;
        let lock = directory.writable_file(std::ffi::OsStr::new("admission.lock"))?;
        lock.lock_exclusive()?;
        let (manifest, created) = bind_file_inputs(&directory, &request, sources, answer)?;
        let result = self.pin_file_inputs(&manifest);
        if result.is_err() && created {
            self.release_input_manifest(&directory, &manifest)?;
        }
        result?;
        let root = self.input_root(&manifest.owner, &manifest.key)?;
        for pointer in manifest.sources.keys() {
            *value
                .pointer_mut(pointer)
                .ok_or_else(|| invalid("file input field disappeared"))? = Value::String(
                root.join(path_hash(&(pointer, &manifest.sources[pointer]))?)
                    .to_str()
                    .ok_or_else(|| invalid("input path is not UTF-8"))?
                    .to_owned(),
            );
        }
        let claim = created.then(|| FileInputClaim {
            run_id: manifest.run_id.clone(),
            pin_nonce: manifest.pin_nonce,
        });
        Ok(FrozenFileInputs {
            run_id: manifest.run_id,
            value,
            claim,
        })
    }

    fn pin_file_inputs(&self, manifest: &Manifest) -> io::Result<()> {
        let access = self.input_access();
        for (pointer, source) in &manifest.sources {
            let pin = pin(manifest, pointer)?;
            if self.host.pin_exists(&access, &pin)? {
                continue;
            }
            match source {
                Source::Target { selection } => {
                    self.host.pin(
                        &access,
                        &FilePinRequest {
                            pin,
                            source: selection.clone(),
                        },
                    )?;
                }
                Source::Local { path } => {
                    // Resolve caller-relative parents only when acquiring a new pin. Replays
                    // remain independent of the original, which may no longer exist. Refuse
                    // links before canonicalization could hide a link followed by `..`.
                    let _ancestors = paths::guard_ancestors(path)?;
                    paths::reject_links(path)?;
                    let source = path.canonicalize()?;
                    let mut transfer = super::new_transfer(&self.directory.join("transfers"))?;
                    super::pack(&source, &mut transfer)?;
                    transfer.rewind()?;
                    self.host.stage(&access, &pin, &mut transfer)?;
                }
                Source::Staged { pin: upload } => {
                    let mut transfer = super::new_transfer(&self.directory.join("transfers"))?;
                    self.host.read_pin(&access, upload, &mut transfer)?;
                    transfer.rewind()?;
                    self.host.stage(&access, &pin, &mut transfer)?;
                }
            }
        }
        Ok(())
    }

    /// Input roots are host-owned manifest selections, never arbitrary strings in a graph value.
    pub fn file_input_roots(&self, owner: &RunId) -> io::Result<Vec<PathBuf>> {
        let directory = self.input_directory()?;
        let mut roots = BTreeSet::new();
        for name in directory.entries()? {
            let name = name?;
            if !name.to_string_lossy().starts_with("run-") {
                continue;
            }
            let manifest: Manifest = serde_json::from_reader(directory.read_file(&name)?)?;
            if manifest.owner == *owner && !released(&directory, &manifest.run_id)? {
                roots.insert(self.input_root(&manifest.owner, &manifest.key)?);
            }
        }
        Ok(roots.into_iter().collect())
    }

    /// Record canonical response authority without changing controller-state path identity.
    pub fn file_input_authority_roots(&self, owner: &RunId) -> io::Result<Vec<PathBuf>> {
        self.file_input_roots(owner)?
            .into_iter()
            .map(|root| {
                // Refuse redirected ancestry before canonicalization can hide it. Windows guards
                // also keep the ancestry unchanged while its ordinary spelling is canonicalized.
                paths::reject_links(&root)?;
                // Startup has not repaired/materialized the mutable key root yet. Anchor its
                // spelling to host-owned ancestry without requiring read access to that root.
                let mut existing = root
                    .parent()
                    .ok_or_else(|| invalid("input root has no parent"))?;
                while !existing.try_exists()? {
                    existing = existing
                        .parent()
                        .ok_or_else(|| invalid("input root has no parent"))?;
                }
                let _guard = paths::Directory::open(existing)?;
                Ok(existing.canonicalize()?.join(
                    root.strip_prefix(existing)
                        .map_err(|_| invalid("invalid input root"))?,
                ))
            })
            .collect()
    }

    /// Rebuild from immutable accepted pins, or fill missing roots before continuation.
    pub fn materialize_file_inputs(
        &self,
        owner: &RunId,
        restart: bool,
    ) -> io::Result<Vec<PathBuf>> {
        let directory = self.input_directory()?;
        let lock = directory.writable_file(std::ffi::OsStr::new("admission.lock"))?;
        lock.lock_exclusive()?;
        let mut roots = Vec::new();
        for name in directory.entries()? {
            let name = name?;
            if !name.to_string_lossy().starts_with("run-") {
                continue;
            }
            let manifest: Manifest = serde_json::from_reader(directory.read_file(&name)?)?;
            if manifest.owner != *owner || released(&directory, &manifest.run_id)? {
                continue;
            }
            let root = self.input_root(&manifest.owner, &manifest.key)?;
            paths::reject_links(&root)?;
            if restart || !root.exists() {
                Self::prepare_input_directory(&self.input_data_directory())?;
                let location = self.input_location(&manifest.owner)?;
                Self::prepare_input_directory(&location)?;
                let stage = crate::native_v2_supervisor::checkpoints::filesystem::private_stage(
                    &location, ".inputs-",
                )?;
                for pointer in manifest.sources.keys() {
                    self.host.materialize(
                        &self.input_access(),
                        &pin(&manifest, pointer)?,
                        &stage
                            .path()
                            .join(path_hash(&(pointer, &manifest.sources[pointer]))?),
                    )?;
                }
                if root.exists() {
                    remove_input_tree(&root)?;
                }
                std::fs::rename(stage.path(), &root)?;
            }
            roots.push(root);
        }
        Ok(roots)
    }

    fn release_input_manifest(
        &self,
        directory: &paths::Directory,
        manifest: &Manifest,
    ) -> io::Result<()> {
        write(
            directory,
            &format!("released-{}", hash(&manifest.run_id)?),
            &true,
        )?;
        for pointer in manifest.sources.keys() {
            self.host
                .release(&self.input_access(), &pin(manifest, pointer)?)?;
        }
        remove_input_tree(&self.input_root(&manifest.owner, &manifest.key)?)
    }

    /// Upload handles are temporary. Accepted inputs have their own immutable private pins.
    pub fn release_staged_inputs(&self, schema: &PayloadType, value: &Value) -> io::Result<()> {
        for upload in staged_uploads(schema, value)? {
            self.host.release_upload(&self.input_access(), &upload)?;
        }
        Ok(())
    }

    pub fn release_file_input_part(&self, run_id: &RunId) -> io::Result<()> {
        self.release_input_part(run_id, None)
    }

    /// Rollback never selects another attempt's manifest, including an idempotent replay.
    pub fn release_file_input_claim(&self, claim: &FileInputClaim) -> io::Result<()> {
        self.release_input_part(&claim.run_id, Some(&claim.pin_nonce))
    }

    fn release_input_part(&self, run_id: &RunId, pin_nonce: Option<&str>) -> io::Result<()> {
        let directory = self.input_directory()?;
        let lock = directory.writable_file(std::ffi::OsStr::new("admission.lock"))?;
        lock.lock_exclusive()?;
        let name = format!("run-{}.json", hash(run_id)?);
        match directory.read_file(std::ffi::OsStr::new(&name)) {
            Ok(file) => {
                let manifest: Manifest = serde_json::from_reader(file)?;
                if manifest.run_id != *run_id {
                    return Err(invalid("input run identity mismatch"));
                }
                if pin_nonce.is_some_and(|nonce| nonce != manifest.pin_nonce) {
                    return Ok(());
                }
                self.release_input_manifest(&directory, &manifest)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Release one recovery lineage; independent library files and publications keep their owners.
    pub fn release_file_inputs(&self, owner: &RunId) -> io::Result<()> {
        let directory = self.input_directory()?;
        let lock = directory.writable_file(std::ffi::OsStr::new("admission.lock"))?;
        lock.lock_exclusive()?;
        for name in directory.entries()? {
            let name = name?;
            if !name.to_string_lossy().starts_with("run-") {
                continue;
            }
            let manifest: Manifest = serde_json::from_reader(directory.read_file(&name)?)?;
            if manifest.owner == *owner {
                self.release_input_manifest(&directory, &manifest)?;
            }
        }
        self.host.reclaim()?;
        Ok(())
    }
}
